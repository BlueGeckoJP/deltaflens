use std::sync::Arc;

use deltaflens_core::DecodedImage;
use eyre::{Result, eyre};
use gpui::RenderImage;
use image::{Frame, RgbaImage};

/// Wraps the pixels without copying them. Both use BGRA, despite the name
/// `RgbaImage`.
pub fn to_render_image(image: DecodedImage) -> Result<Arc<RenderImage>> {
    let (width, height) = (image.width(), image.height());
    let buffer = RgbaImage::from_raw(width, height, image.into_pixels())
        .ok_or_else(|| eyre!("pixel buffer does not match {width}x{height}"))?;
    Ok(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}
