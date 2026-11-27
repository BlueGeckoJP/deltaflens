use std::sync::Arc;

use deltaflens_core::DecodedImage;
use eyre::{Result, eyre};
use gpui::{App, RenderImage};
use image::{Frame, RgbaImage};

/// Wraps the pixels without copying them. Both use BGRA, despite the name
/// `RgbaImage`.
pub fn to_render_image(image: DecodedImage) -> Result<Arc<RenderImage>> {
    let (width, height) = (image.width(), image.height());
    let buffer = RgbaImage::from_raw(width, height, image.into_pixels())
        .ok_or_else(|| eyre!("pixel buffer does not match {width}x{height}"))?;
    Ok(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

/// Frees the textures of `images`, if they have any.
pub fn release_textures(images: Vec<Arc<RenderImage>>, cx: &mut App) {
    if images.is_empty() {
        return;
    }
    // Wait until no window is being updated or drawn, as `drop_image` would
    // miss it.
    cx.defer(move |cx| {
        for image in images {
            cx.drop_image(image, None);
        }
    });
}
