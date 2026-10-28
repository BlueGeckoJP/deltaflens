//! Small versions of images for the explorer.

use std::path::Path;

use eyre::{Result, eyre};
use image::{RgbaImage, imageops};

use super::{DecodedImage, decode};

/// Decodes `path` and shrinks it to fit in a `max_size` square, keeping the
/// aspect ratio. Images that already fit are not enlarged.
///
/// # Errors
///
/// Returns an error if [`decode`] fails.
pub fn thumbnail(path: &Path, max_size: u32) -> Result<DecodedImage> {
    let image = decode(path)?;
    let (width, height) = fit_within(image.width, image.height, max_size);
    if (width, height) == (image.width, image.height) {
        return Ok(image);
    }
    // The pixels are BGRA, but averaging does not depend on the channel order.
    let buffer = RgbaImage::from_raw(image.width, image.height, image.pixels)
        .ok_or_else(|| eyre!("pixel buffer does not match the image size"))?;
    let small = imageops::thumbnail(&buffer, width, height);
    Ok(DecodedImage {
        width,
        height,
        pixels: small.into_raw(),
    })
}

/// The size of a `width` x `height` image shrunk to fit in a `max_size`
/// square, at least 1 x 1.
pub(crate) fn fit_within(width: u32, height: u32, max_size: u32) -> (u32, u32) {
    let longest = width.max(height);
    if longest <= max_size {
        return (width, height);
    }
    let scale = |side: u32| {
        let scaled =
            (u64::from(side) * u64::from(max_size) + u64::from(longest) / 2) / u64::from(longest);
        // Not larger than `max_size`, so it fits in u32.
        u32::try_from(scaled).unwrap_or(max_size).max(1)
    };
    (scale(width), scale(height))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const RED: [u8; 4] = [0, 0, 255, 255];

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn fit_within_keeps_aspect_ratio() {
        assert_eq!(fit_within(4000, 2000, 256), (256, 128));
        assert_eq!(fit_within(1080, 1920, 256), (144, 256));
    }

    #[test]
    fn fit_within_does_not_enlarge() {
        assert_eq!(fit_within(100, 50, 256), (100, 50));
        assert_eq!(fit_within(256, 256, 256), (256, 256));
    }

    #[test]
    fn fit_within_keeps_thin_images_visible() {
        assert_eq!(fit_within(10_000, 1, 256), (256, 1));
    }

    #[test]
    fn thumbnail_shrinks_large_images() {
        let image = thumbnail(&fixture("red-3x2.png"), 2).unwrap();
        assert_eq!((image.width(), image.height()), (2, 1));
        assert_eq!(image.into_pixels(), RED.repeat(2));
    }

    #[test]
    fn thumbnail_keeps_small_images() {
        let image = thumbnail(&fixture("red-3x2.jxl"), 256).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn thumbnail_of_corrupt_image_fails() {
        assert!(thumbnail(&fixture("truncated.png"), 256).is_err());
    }
}
