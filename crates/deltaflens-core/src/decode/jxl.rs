//! JPEG XL decoding with jxl-oxide, which `image` does not support.

use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use eyre::{Result, WrapErr, bail, ensure, eyre};
use jxl_oxide::{
    EnumColourEncoding, ExtraChannel, FrameHeader, JxlImage, PixelFormat, Render, RenderingIntent,
    color::ColourEncoding, image::BitDepth,
};
use jxl_render::ImageBuffer;

use super::{
    DecodedImage,
    animation::{AnimationInfo, DecodedFrame, FrameSource, plays, usable_profile},
    convert_to_srgb, describes_srgb, limits,
};

/// A bare JPEG XL codestream.
const CODESTREAM_SIGNATURE: [u8; 2] = [0xff, 0x0a];
/// A codestream in the ISOBMFF-based container.
const CONTAINER_SIGNATURE: [u8; 12] = [
    0x00, 0x00, 0x00, 0x0c, b'J', b'X', b'L', b' ', 0x0d, 0x0a, 0x87, 0x0a,
];
/// How many bytes [`has_signature`] needs to see.
pub(super) const SIGNATURE_LEN: usize = CONTAINER_SIGNATURE.len();

/// Returns whether the file starts with a JPEG XL signature.
pub(super) fn is_jxl(path: &Path) -> Result<bool> {
    let mut header = Vec::with_capacity(SIGNATURE_LEN);
    File::open(path)?
        .take(SIGNATURE_LEN as u64)
        .read_to_end(&mut header)?;
    Ok(has_signature(&header))
}

/// Returns whether `header`, the start of a file, is a JPEG XL signature.
pub(super) fn has_signature(header: &[u8]) -> bool {
    header.starts_with(&CODESTREAM_SIGNATURE) || header.starts_with(&CONTAINER_SIGNATURE)
}

/// Decodes the first frame in sRGB, with the orientation applied.
pub(super) fn decode(path: &Path) -> Result<DecodedImage> {
    Prepared::new(open(path)?)?.render(0, path)
}

/// Opens an animation to decode a frame at a time, or returns `None` for an
/// image with fewer than two frames.
pub(super) fn open_animation(path: &Path) -> Result<Option<JxlFrames>> {
    let image = open(path)?;
    let Some(animation) = &image.image_header().metadata.animation else {
        return Ok(None);
    };
    let ticks_per_second = (animation.tps_numerator, animation.tps_denominator);
    let loop_count = plays(animation.num_loops);
    let frame_count = image.num_loaded_keyframes();
    if frame_count < 2 {
        return Ok(None);
    }
    let independent = frames_are_independent(&image);
    let mut prepared = Prepared::new(image)?;
    // Checked once here, as every frame would warn of it otherwise.
    prepared.icc = usable_profile(prepared.icc.take(), path);
    let info = AnimationInfo {
        width: prepared.image.width(),
        height: prepared.image.height(),
        frame_count,
        loop_count,
    };
    Ok(Some(JxlFrames {
        prepared,
        path: path.to_path_buf(),
        info,
        ticks_per_second,
        independent,
        next: 0,
    }))
}

/// Reads the whole file.
fn open(path: &Path) -> Result<JxlImage> {
    let image = JxlImage::builder()
        .open(path)
        .map_err(|error| eyre!(error))?;
    if !image.is_loading_done() {
        bail!("the file is truncated");
    }
    Ok(image)
}

/// An image set up to render its frames in sRGB, or in the color space of its
/// ICC profile.
struct Prepared {
    image: JxlImage,
    /// Whether all channels are stored as 8-bit integers.
    eight_bit: bool,
    /// The color space to convert from, if it is given by an ICC profile.
    icc: Option<Vec<u8>>,
}

impl Prepared {
    fn new(mut image: JxlImage) -> Result<Self> {
        let metadata = &image.image_header().metadata;
        let eight_bit = is_eight_bit(metadata.bit_depth)
            && metadata
                .ec_info
                .iter()
                .all(|channel| is_eight_bit(channel.bit_depth));
        // A color space given by an ICC profile is converted afterwards, as
        // for the other formats. Converting it with the moxcms support of
        // jxl-oxide gave wrong colors.
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
            // jxl-oxide renders in the embedded profile, or in sRGB if it
            // cannot, such as for a profile it cannot write as an enum color
            // encoding in an image stored in XYB. The profile it makes for the
            // rendered pixels from an enum color encoding gave wrong colors,
            // so the embedded one is used.
            ColourEncoding::IccProfile(_) if describes_srgb(&image.rendered_icc()) => None,
            ColourEncoding::IccProfile(_) => image.original_icc().map(<[u8]>::to_vec),
        };

        let format = image.pixel_format();
        if matches!(format, PixelFormat::Cmyk | PixelFormat::Cmyka) {
            bail!("CMYK images are not supported");
        }
        limits().reserve(u64::from(image.width()) * u64::from(image.height()) * 4)?;
        Ok(Self {
            image,
            eight_bit,
            icc,
        })
    }

    /// Renders the keyframe at `index` in sRGB, with the orientation
    /// applied.
    fn render(&self, index: usize, path: &Path) -> Result<DecodedImage> {
        let (width, height) = (self.image.width(), self.image.height());
        let direct = self
            .image
            .frame_header(index)
            .is_some_and(|header| covers_image(header, width, height));
        let render = self
            .image
            .render_frame(index)
            .map_err(|error| eyre!(error))?;
        let output = Output {
            width,
            height,
            direct,
            eight_bit: self.eight_bit,
            icc: self.icc.as_deref(),
        };
        to_image(&render, &output, path)
    }
}

/// The frames of an animation, rendered a keyframe at a time.
///
/// jxl-render keeps every keyframe it has rendered until the image is dropped
/// or its region changes, as later frames may be drawn over earlier ones. So
/// when no frame is kept for later frames ([`frames_are_independent`]), the
/// kept renders are dropped after each frame, which keeps only one frame in
/// memory. Otherwise they are kept, as dropping them would make each later
/// frame render again every frame it is drawn over, back to the first; memory
/// then grows with each frame rendered for the first time, up to all
/// keyframes of the animation, several times the size of their BGRA pixels.
pub(super) struct JxlFrames {
    prepared: Prepared,
    path: PathBuf,
    info: AnimationInfo,
    /// The numerator and denominator of the ticks per second, the unit of
    /// frame durations.
    ticks_per_second: (u32, u32),
    /// Whether no frame is kept for later frames.
    independent: bool,
    /// The index of the keyframe [`FrameSource::next_frame`] gives.
    next: usize,
}

impl FrameSource for JxlFrames {
    fn info(&self) -> AnimationInfo {
        self.info
    }

    fn next_frame(&mut self) -> Result<Option<DecodedFrame>> {
        let index = self.next;
        if index == self.info.frame_count {
            return Ok(None);
        }
        let image = self
            .prepared
            .render(index, &self.path)
            .wrap_err_with(|| format!("failed to decode frame {index}"))?;
        if self.independent {
            // Setting the region, even to the same one, drops the renders.
            let image = &mut self.prepared.image;
            image.set_image_region(image.current_image_region());
        }
        let ticks = self
            .prepared
            .image
            .frame_header(index)
            .map_or(0, |header| header.duration);
        self.next += 1;
        Ok(Some(DecodedFrame {
            index,
            image,
            delay: ticks_to_duration(ticks, self.ticks_per_second),
        }))
    }

    fn seek(&mut self, frame_index: usize) -> Result<()> {
        ensure!(
            frame_index < self.info.frame_count,
            "frame {frame_index} is out of {} frames",
            self.info.frame_count
        );
        // jxl-render renders whatever earlier frames a keyframe needs.
        self.next = frame_index;
        Ok(())
    }
}

/// Returns whether no frame of `image` is kept for later frames to be drawn
/// over or to use otherwise, so that each keyframe renders on its own.
///
/// jxl-render makes a keyframe depend on every frame saved in a reference or
/// LF slot before it, which a frame is if it can be referenced or is an LF
/// frame.
fn frames_are_independent(image: &JxlImage) -> bool {
    (0..image.num_loaded_frames()).all(|index| {
        image.frame(index).is_some_and(|frame| {
            let header = frame.header();
            !header.can_reference() && header.lf_level == 0
        })
    })
}

/// How long `ticks` last at `ticks_per_second`, given as a numerator and a
/// denominator.
fn ticks_to_duration(ticks: u32, (numerator, denominator): (u32, u32)) -> Duration {
    if numerator == 0 {
        return Duration::ZERO;
    }
    let nanos = u128::from(ticks) * u128::from(denominator) * 1_000_000_000 / u128::from(numerator);
    Duration::from_nanos(u64::try_from(nanos).unwrap_or(u64::MAX))
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
    fn frames_drawn_over_earlier_ones_are_not_independent() {
        let open = |name| JxlImage::builder().open(fixture(name)).unwrap();
        assert!(frames_are_independent(&open("anim-rgb.jxl")));
        assert!(frames_are_independent(&open("red-3x2.jxl")));
        assert!(!frames_are_independent(&open("anim-blend.jxl")));
    }

    #[test]
    fn ticks_become_durations() {
        let ms = Duration::from_millis;
        assert_eq!(ticks_to_duration(1, (30, 3)), ms(100));
        assert_eq!(ticks_to_duration(3, (1000, 1)), ms(3));
        // 30000 / 1001 ticks per second, as in NTSC video, rounded down.
        assert_eq!(
            ticks_to_duration(1, (30_000, 1001)),
            Duration::from_nanos(33_366_666)
        );
        assert_eq!(ticks_to_duration(0, (10, 1)), Duration::ZERO);
        assert_eq!(ticks_to_duration(5, (0, 1)), Duration::ZERO);
        assert_eq!(
            ticks_to_duration(u32::MAX, (1, u32::MAX)),
            Duration::from_nanos(u64::MAX)
        );
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
