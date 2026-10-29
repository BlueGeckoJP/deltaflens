//! JPEG XL decoding with jxl-oxide, which `image` does not support.

use std::{fs::File, io::Read, path::Path};

use eyre::{Result, bail, eyre};
use jxl_oxide::{
    EnumColourEncoding, JxlImage, PixelFormat, RenderingIntent, color::ColourEncoding,
};

use super::{DecodedImage, convert_to_srgb, limits};

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
