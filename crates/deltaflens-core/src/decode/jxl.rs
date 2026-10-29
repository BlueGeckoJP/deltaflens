//! JPEG XL decoding with jxl-oxide, which `image` does not support.

use std::{fs::File, io::Read, path::Path};

use eyre::{Result, bail, eyre};
use jxl_oxide::{
    EnumColourEncoding, JxlImage, PixelFormat, RenderingIntent, color::ColourEncoding,
};

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

    let render = image.render_frame(0).map_err(|error| eyre!(error))?;
    let mut stream = render.stream();
    let channels = stream.channels() as usize;
    let mut samples = vec![0u8; width as usize * height as usize * channels];
    stream.write_to_buffer(&mut samples);

    let mut pixels: Vec<u8> = match channels {
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
    };
    if let Some(icc) = icc
        && let Err(error) = convert_to_srgb(&mut pixels, &icc)
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
}
