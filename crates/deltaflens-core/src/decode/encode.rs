//! PNG encoding, for the thumbnail cache and the clipboard.

use std::io::Write;

use eyre::Result;

use super::DecodedImage;

/// Writes `image` as an 8-bit RGBA PNG with the given tEXt chunks.
pub(crate) fn encode_png(
    writer: impl Write,
    image: &DecodedImage,
    text: &[(&str, String)],
    compression: png::Compression,
) -> Result<()> {
    let mut encoder = png::Encoder::new(writer, image.width, image.height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(compression);
    for (keyword, value) in text {
        encoder.add_text_chunk((*keyword).to_owned(), value.clone())?;
    }
    let mut writer = encoder.write_header()?;
    let mut rgba = image.pixels.clone();
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    writer.write_image_data(&rgba)?;
    writer.finish()?;
    Ok(())
}
