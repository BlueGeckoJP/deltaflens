//! JPEG XL decoding with jxl-oxide, which `image` does not support.

use std::{fs::File, io::Read, path::Path};

use eyre::{Result, bail, eyre};
use jxl_oxide::{
    EnumColourEncoding, ExtraChannel, FrameHeader, JxlImage, PixelFormat, Render, RenderingIntent,
    color::ColourEncoding, image::BitDepth,
};
use jxl_render::ImageBuffer;

use super::{DecodedImage, convert_to_srgb, describes_srgb, limits};

/// A bare JPEG XL codestream.
const CODESTREAM_SIGNATURE: [u8; 2] = [0xff, 0x0a];
/// A codestream in the ISOBMFF-based container.
const CONTAINER_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
];

/// Returns whether the file starts with a JPEG XL signature.
pub(super) fn is_jxl(path: &Path) -> Result<bool> {
    let mut header = Vec::with_capacity(CONTAINER_SIGNATURE.len());
    File::open(path)?
        .take(CONTAINER_SIGNATURE.len() as u64)
        .read_to_end(&mut header)?;
    Ok(header.starts_with(&CODESTREAM_SIGNATURE) || header.starts_with(&CONTAINER_SIGNATURE))
}

/// Decodes the first frame in sRGB, with the orientation applied.
pub(super) fn decode(path: &Path) -> Result<DecodedImage> {
    let mut image = JxlImage::builder()
        .open(path)
        .map_err(|error| eyre!(error))?;
    if !image.is_loading_done() {
        bail!("the file is truncated");
    }
    let metadata = &image.image_header().metadata;
    let eight_bit = is_eight_bit(metadata.bit_depth)
        && metadata
            .ec_info
            .iter()
            .all(|channel| is_eight_bit(channel.bit_depth));
    // A color space given by an ICC profile is converted afterwards, as for
    // the other formats. Converting it with the moxcms support of jxl-oxide
    // gave wrong colors.
    let icc = match metadata.colour_encoding {
        ColourEncoding::Enum(_) => {
            let srgb = if metadata.grayscale() {
                EnumColourEncoding::gray_srgb(RenderingIntent::Relative)
            } else {
                EnumColourEncoding::srgb(RenderingIntent::Relative)
            };
            image.request_color_encoding(srgb);
            None
        }
        // jxl-oxide renders in the embedded profile, or in sRGB if it cannot,
        // such as for a profile it cannot write as an enum color encoding in
        // an image stored in XYB. The profile it makes for the rendered pixels
        // from an enum color encoding gave wrong colors, so the embedded one
        // is used.
        ColourEncoding::IccProfile(_) if describes_srgb(&image.rendered_icc()) => None,
        ColourEncoding::IccProfile(_) => image.original_icc().map(<[u8]>::to_vec),
    };

    let format = image.pixel_format();
    if matches!(format, PixelFormat::Cmyk | PixelFormat::Cmyka) {
        bail!("CMYK images are not supported");
    }
    let (width, height) = (image.width(), image.height());
    limits().reserve(u64::from(width) * u64::from(height) * 4)?;

    let direct = image
        .frame_header(0)
        .is_some_and(|header| covers_image(header, width, height));
    let render = image.render_frame(0).map_err(|error| eyre!(error))?;
    let output = Output {
        width,
        height,
        direct,
        eight_bit,
        icc: icc.as_deref(),
    };
    to_image(&render, &output, path)
}

/// Returns whether samples of `bit_depth` are 8-bit integers.
fn is_eight_bit(bit_depth: BitDepth) -> bool {
    matches!(bit_depth, BitDepth::IntegerSample { bits_per_sample: 8 })
}

/// Returns whether the frame of `header` lies exactly over the image, so that
/// its samples are at the same places as the pixels of the image.
fn covers_image(header: &FrameHeader, width: u32, height: u32) -> bool {
    header.x0 == 0 && header.y0 == 0 && header.width == width && header.height == height
}

/// What a rendered frame becomes.
struct Output<'a> {
    width: u32,
    height: u32,
    /// Whether the frame lies exactly over the image.
    direct: bool,
    /// Whether all channels are stored as 8-bit integers.
    eight_bit: bool,
    /// The color space to convert from, if it is given by an ICC profile.
    icc: Option<&'a [u8]>,
}

/// Converts a rendered frame to BGRA in sRGB.
fn to_image(render: &Render, output: &Output, path: &Path) -> Result<DecodedImage> {
    let (width, height) = (output.width, output.height);
    let mut pixels = match grids_to_rgba(render, output) {
        Some(pixels) => pixels,
        None => stream_to_rgba(render, width, height)?,
    };
    if let Some(icc) = output.icc
        && let Err(error) = convert_to_srgb(&mut pixels, icc)
    {
        tracing::warn!(
            "{}: colors are not converted to sRGB: {error:#}",
            path.display()
        );
    }
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(DecodedImage {
        width,
        height,
        pixels,
    })
}

/// The 8-bit RGBA pixels of a rendered frame, read from its channels row by
/// row, or `None` where only the stream of jxl-oxide gets them right: for a
/// rotated or flipped image, a frame that does not lie exactly over the image,
/// spot colors, or integer samples of other than 8 bits. The stream is much
/// slower, as it reads each sample on its own.
fn grids_to_rgba(render: &Render, output: &Output) -> Option<Vec<u8>> {
    let (width, height) = (output.width as usize, output.height as usize);
    let (extra_channels, extra_grids) = render.extra_channels();
    if render.orientation() != 1
        || !output.direct
        || extra_channels.iter().any(ExtraChannel::is_spot_colour)
    {
        return None;
    }
    let colors: [&ImageBuffer; 3] = match render.color_channels() {
        [gray] => [gray; 3],
        [red, green, blue] => [red, green, blue],
        _ => return None,
    };
    let alpha = extra_channels
        .iter()
        .position(ExtraChannel::is_alpha)
        .map(|index| &extra_grids[index]);
    let channels: Vec<(usize, &ImageBuffer)> = colors
        .into_iter()
        .enumerate()
        .chain(alpha.map(|alpha| (3, alpha)))
        .collect();
    let usable = |grid: &ImageBuffer| {
        grid.width() == width
            && grid.height() == height
            && (output.eight_bit || matches!(grid, ImageBuffer::F32(_)))
    };
    if !channels.iter().all(|(_, grid)| usable(grid)) {
        return None;
    }

    // The same rounding as the stream.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let from_f32 = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8;
    let from_int = |v: i32| u8::try_from(v.clamp(0, 255)).unwrap_or(u8::MAX);
    let mut pixels = vec![u8::MAX; width * height * 4];
    for (y, row) in pixels.chunks_exact_mut(width * 4).enumerate() {
        for (channel, grid) in &channels {
            let samples = row[*channel..].iter_mut().step_by(4);
            match grid {
                ImageBuffer::F32(grid) => {
                    for (sample, &v) in samples.zip(grid.get_row(y)) {
                        *sample = from_f32(v);
                    }
                }
                ImageBuffer::I32(grid) => {
                    for (sample, &v) in samples.zip(grid.get_row(y)) {
                        *sample = from_int(v);
                    }
                }
                ImageBuffer::I16(grid) => {
                    for (sample, &v) in samples.zip(grid.get_row(y)) {
                        *sample = from_int(i32::from(v));
                    }
                }
            }
        }
    }
    Some(pixels)
}

/// The 8-bit RGBA pixels of a rendered frame, through the stream of
/// jxl-oxide, with the orientation applied.
fn stream_to_rgba(render: &Render, width: u32, height: u32) -> Result<Vec<u8>> {
    let mut stream = render.stream();
    let channels = stream.channels() as usize;
    let mut samples = vec![0u8; width as usize * height as usize * channels];
    stream.write_to_buffer(&mut samples);
    Ok(match channels {
        1 => samples.iter().flat_map(|&v| [v, v, v, u8::MAX]).collect(),
        2 => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[v, a]| [v, v, v, a])
            .collect(),
        3 => samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|&[r, g, b]| [r, g, b, u8::MAX])
            .collect(),
        4 => samples,
        _ => bail!("unexpected number of channels: {channels}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn icc_profile_rendered_as_srgb_is_not_converted_again() {
        // Lossy, so stored in XYB, with an ICC profile of Display P3 primaries
        // and tone curves given as tables. jxl-oxide cannot render in it and
        // renders in sRGB instead.
        let path = fixture("p3-table-curve-xyb-8x8.jxl");
        let image = JxlImage::builder().open(&path).unwrap();
        assert!(describes_srgb(&image.rendered_icc()));

        // The pixels were (234, 51, 35) in the profile.
        let mut expected = vec![234, 51, 35, 255];
        convert_to_srgb(&mut expected, image.original_icc().unwrap()).unwrap();
        let decoded = decode(&path).unwrap();
        for pixel in decoded.pixels().as_chunks::<4>().0 {
            let [b, g, r, a] = *pixel;
            let close = [r, g, b, a]
                .iter()
                .zip(&expected)
                .all(|(&actual, &expected)| actual.abs_diff(expected) <= 6);
            assert!(close, "{:?} is not close to {expected:?}", [r, g, b, a]);
        }
    }

    #[test]
    fn channels_give_the_same_pixels_as_the_stream() {
        for name in [
            "red-3x2.jxl",
            "rgba-2x2.jxl",
            "gray-2x2.jxl",
            "p3-red-8x8.jxl",
            "progressive-256.jxl",
        ] {
            let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(name);
            let mut image = JxlImage::builder().open(&path).unwrap();
            image.request_color_encoding(EnumColourEncoding::srgb(RenderingIntent::Relative));
            let render = image.render_frame(0).unwrap();
            let (width, height) = (image.width(), image.height());
            let output = Output {
                width,
                height,
                direct: true,
                eight_bit: true,
                icc: None,
            };
            assert_eq!(
                grids_to_rgba(&render, &output).as_ref(),
                Some(&stream_to_rgba(&render, width, height).unwrap()),
                "{name}"
            );
        }
    }

    #[test]
    fn rotated_image_goes_through_the_stream() {
        // Stored as 16x8 and rotated by the EXIF orientation, recompressed
        // from the JPEG.
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/orientation-6.jxl");
        let image = JxlImage::builder().open(&path).unwrap();
        assert_ne!(image.image_header().metadata.orientation, 1);
        let image = decode(&path).unwrap();
        assert_eq!((image.width(), image.height()), (8, 16));
        // Red on top, blue below, as the JPEG.
        let pixel = |x: u32, y: u32| image.pixels().as_chunks::<4>().0[(y * 8 + x) as usize];
        assert!(pixel(4, 3)[2] > 200 && pixel(4, 3)[0] < 50);
        assert!(pixel(4, 12)[0] > 200 && pixel(4, 12)[2] < 50);
    }
}
