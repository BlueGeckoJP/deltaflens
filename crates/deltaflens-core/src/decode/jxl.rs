//! JPEG XL decoding with jxl-oxide, which `image` does not support.

use std::{fs::File, io::Read, path::Path};

use eyre::{Result, bail, eyre};
use image::Limits;
use jxl_oxide::{
    EnumColourEncoding, JxlImage, PixelFormat, RenderingIntent, color::ColourEncoding,
};

use super::DecodedImage;

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
///
/// Images whose color space is given by an embedded ICC profile are left in
/// that color space, because converting them needs a color management system.
pub(super) fn decode(path: &Path) -> Result<DecodedImage> {
    let mut image = JxlImage::builder()
        .open(path)
        .map_err(|error| eyre!(error))?;
    if !image.is_loading_done() {
        bail!("the file is truncated");
    }
    let metadata = &image.image_header().metadata;
    match metadata.colour_encoding {
        ColourEncoding::Enum(_) => {
            let srgb = if metadata.grayscale() {
                EnumColourEncoding::gray_srgb(RenderingIntent::Relative)
            } else {
                EnumColourEncoding::srgb(RenderingIntent::Relative)
            };
            image.request_color_encoding(srgb);
        }
        ColourEncoding::IccProfile(_) => {
            tracing::warn!(
                "{}: colors are not converted to sRGB, as the color space is an ICC profile",
                path.display()
            );
        }
    }

    let format = image.pixel_format();
    if matches!(format, PixelFormat::Cmyk | PixelFormat::Cmyka) {
        bail!("CMYK images are not supported");
    }
    let (width, height) = (image.width(), image.height());
    Limits::default().reserve(u64::from(width) * u64::from(height) * 4)?;

    let render = image.render_frame(0).map_err(|error| eyre!(error))?;
    let mut stream = render.stream();
    let channels = stream.channels() as usize;
    let mut samples = vec![0u8; width as usize * height as usize * channels];
    stream.write_to_buffer(&mut samples);

    let pixels = match channels {
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
            .flat_map(|&[r, g, b]| [b, g, r, u8::MAX])
            .collect(),
        4 => {
            for pixel in samples.as_chunks_mut::<4>().0 {
                pixel.swap(0, 2);
            }
            samples
        }
        _ => bail!("unexpected number of channels: {channels}"),
    };
    Ok(DecodedImage {
        width,
        height,
        pixels,
    })
}
