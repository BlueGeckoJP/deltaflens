//! Smaller versions of images shown below their size, made with a Lanczos3
//! filter so that fine patterns do not alias as the GPU would make them.

use eyre::{Result, WrapErr};
use fast_image_resize::{
    FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer,
    images::{Image, ImageRef},
};

use crate::DecodedImage;

/// The ratio between two sizes that downscaled images are made at. Scales in
/// between show the next larger size, shrunk a little by the GPU, so that
/// zooming does not make a new image at every step.
const STEP: f32 = 1.189_207_1; // 2^(1/4)

/// The size to downscale a `width` x `height` image to, for showing it at
/// `scale` device pixels per image pixel, or `None` if the image itself is to
/// be shown.
///
/// The scale is rounded up to a power of [`STEP`], so the size is at least
/// the size shown and at most [`STEP`] times larger. Each side is at least 1.
#[must_use]
pub fn downscaled_size(width: u32, height: u32, scale: f32) -> Option<(u32, u32)> {
    if scale.is_nan() || scale <= 0.0 {
        return None;
    }
    // Scales that are powers of the step come out a little off; they must
    // not round up to the next step.
    let steps = (scale.ln() / STEP.ln() - 1e-3).ceil();
    if steps >= 0.0 {
        return None;
    }
    let level = STEP.powf(steps);
    // Images are at most a few hundred thousand pixels wide, which f32 holds
    // well enough for a size to show.
    #[expect(clippy::cast_precision_loss)]
    let scaled = |side: u32| (side as f32 * level).round().max(1.0);
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let size = (scaled(width) as u32, scaled(height) as u32);
    (size != (width, height)).then_some(size)
}

/// Downscales `size` BGRA `pixels` to `new_size` with a Lanczos3 filter.
///
/// The colors are weighted by alpha, so that transparent pixels do not bleed
/// their color into the edges of opaque ones.
///
/// # Errors
///
/// Returns an error if `pixels` does not match `size`.
pub fn downscale(
    pixels: &[u8],
    (width, height): (u32, u32),
    (new_width, new_height): (u32, u32),
) -> Result<DecodedImage> {
    let source = ImageRef::new(width, height, pixels, PixelType::U8x4)
        .wrap_err_with(|| format!("pixel buffer does not match {width}x{height}"))?;
    let mut target = Image::new(new_width, new_height, PixelType::U8x4);
    // Alpha is the last channel in BGRA as in RGBA, and the filter does not
    // depend on the order of the others.
    let options = ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3));
    Resizer::new()
        .resize(&source, &mut target, &options)
        .wrap_err("failed to downscale")?;
    Ok(DecodedImage::new(new_width, new_height, target.into_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_of_one_or_more_shows_the_image() {
        assert_eq!(downscaled_size(3840, 2160, 1.0), None);
        assert_eq!(downscaled_size(3840, 2160, 2.5), None);
    }

    #[test]
    fn scale_just_below_one_rounds_up_to_the_image() {
        assert_eq!(downscaled_size(3840, 2160, 0.95), None);
    }

    #[test]
    fn power_of_step_is_kept() {
        assert_eq!(downscaled_size(3840, 2160, 0.25), Some((960, 540)));
        assert_eq!(downscaled_size(3840, 2160, 0.5), Some((1920, 1080)));
    }

    #[test]
    fn scale_rounds_up_to_the_next_step() {
        // 0.3 lies between 2^(-7/4) ≈ 0.297 and 2^(-6/4) ≈ 0.354.
        assert_eq!(downscaled_size(4000, 2000, 0.3), Some((1414, 707)));
    }

    #[test]
    fn nearby_scales_share_a_size() {
        assert_eq!(
            downscaled_size(3840, 2160, 0.31),
            downscaled_size(3840, 2160, 0.35)
        );
    }

    #[test]
    fn thin_image_keeps_a_pixel() {
        assert_eq!(downscaled_size(10_000, 1, 0.01), Some((110, 1)));
    }

    const RED: [u8; 4] = [0, 0, 255, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        image.pixels().as_chunks::<4>().0[(y * image.width() + x) as usize]
    }

    #[test]
    fn downscale_keeps_flat_color() {
        let image = downscale(&RED.repeat(64 * 32), (64, 32), (16, 8)).unwrap();
        assert_eq!((image.width(), image.height()), (16, 8));
        assert!(image.pixels().as_chunks::<4>().0.iter().all(|&p| p == RED));
    }

    #[test]
    fn downscale_keeps_halves_apart() {
        // Red on the left half, blue on the right half.
        let row = [RED.repeat(32), BLUE.repeat(32)].concat();
        let image = downscale(&row.repeat(32), (64, 32), (16, 8)).unwrap();
        assert_eq!(pixel(&image, 2, 4), RED);
        assert_eq!(pixel(&image, 13, 4), BLUE);
    }

    #[test]
    fn downscale_averages_fine_stripes() {
        // One-pixel black and white columns turn gray, instead of aliasing to
        // either.
        let row = [[0, 0, 0, 255], [255, 255, 255, 255]].concat().repeat(32);
        let image = downscale(&row.repeat(64), (64, 64), (16, 16)).unwrap();
        let [b, g, r, a] = pixel(&image, 8, 8);
        for channel in [b, g, r] {
            assert!(channel.abs_diff(128) <= 8, "{channel}");
        }
        assert_eq!(a, 255);
    }

    #[test]
    fn downscale_does_not_bleed_transparent_color() {
        // Clear black pixels next to red ones must not darken them.
        let row = [RED.repeat(32), CLEAR.repeat(32)].concat();
        let image = downscale(&row.repeat(32), (64, 32), (16, 8)).unwrap();
        let [b, g, r, a] = pixel(&image, 8, 4);
        assert!(a > 0 && a < 255, "{a}");
        assert!(r >= 250 && b <= 5 && g <= 5, "{:?}", [b, g, r, a]);
    }

    #[test]
    fn downscale_rejects_wrong_buffer() {
        assert!(downscale(&RED.repeat(10), (64, 32), (16, 8)).is_err());
    }

    #[test]
    fn invalid_scale_shows_the_image() {
        assert_eq!(downscaled_size(3840, 2160, 0.0), None);
        assert_eq!(downscaled_size(3840, 2160, -1.0), None);
        assert_eq!(downscaled_size(3840, 2160, f32::NAN), None);
    }
}
