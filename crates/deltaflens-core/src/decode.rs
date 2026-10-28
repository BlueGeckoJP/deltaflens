mod jxl;
mod thumbnail;

use std::path::Path;

use eyre::{Result, WrapErr};
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Limits};

pub use self::thumbnail::thumbnail;

/// A decoded image with 8-bit BGRA pixels, the layout gpui's `RenderImage`
/// expects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl DecodedImage {
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Row-major BGRA pixels, 4 bytes each.
    #[must_use]
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    #[must_use]
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }
}

/// Decodes the image at `path`, detecting the format from the file contents
/// rather than the extension.
///
/// The orientation is applied, images with more than 8 bits per channel are
/// converted to 8 bits, and only the first frame of an animation is decoded.
/// JPEG XL images are converted to sRGB, unless their color space is given by
/// an ICC profile.
///
/// # Errors
///
/// Returns an error if the file cannot be read, its format is not supported,
/// or its data is corrupt.
pub fn decode(path: &Path) -> Result<DecodedImage> {
    decode_inner(path).wrap_err_with(|| format!("failed to decode {}", path.display()))
}

fn decode_inner(path: &Path) -> Result<DecodedImage> {
    if jxl::is_jxl(path)? {
        return jxl::decode(path);
    }
    let mut decoder = ImageReader::open(path)?
        .with_guessed_format()?
        .into_decoder()?;
    // `ImageReader::decode` checks this, but `into_decoder` does not.
    Limits::default().reserve(decoder.total_bytes())?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);

    let mut buffer = image.into_rgba8();
    for pixel in buffer.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(DecodedImage {
        width: buffer.width(),
        height: buffer.height(),
        pixels: buffer.into_raw(),
    })
}

/// Returns whether the extension of `path` (case-insensitive) belongs to a
/// format that [`decode`] can read.
#[must_use]
pub fn is_supported_image(path: &Path) -> bool {
    path.extension().is_some_and(|extension| {
        extension.eq_ignore_ascii_case("jxl")
            || ImageFormat::from_extension(extension).is_some_and(|format| format.reading_enabled())
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    const RED: [u8; 4] = [0, 0, 255, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        image.pixels().as_chunks::<4>().0[(y * image.width() + x) as usize]
    }

    /// For lossy formats.
    fn assert_close(actual: [u8; 4], expected: [u8; 4]) {
        let close = actual
            .iter()
            .zip(expected)
            .all(|(&a, e)| a.abs_diff(e) <= 8);
        assert!(close, "{actual:?} is not close to {expected:?}");
    }

    #[test]
    fn decode_png_as_bgra() {
        let image = decode(&fixture("red-3x2.png")).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn decode_lossless_webp_as_bgra() {
        let image = decode(&fixture("red-3x2.webp")).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn decode_jpeg_as_bgra() {
        let image = decode(&fixture("red-3x2.jpg")).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        for &pixel in image.pixels().as_chunks::<4>().0 {
            assert_close(pixel, RED);
        }
    }

    #[test]
    fn decode_detects_format_from_contents() {
        let image = decode(&fixture("png-named.jpg")).unwrap();
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn decode_applies_exif_orientation() {
        // Stored as 16x8 with red on the left and blue on the right; rotating
        // it 90° clockwise puts red on top.
        let image = decode(&fixture("orientation-6.jpg")).unwrap();
        assert_eq!((image.width(), image.height()), (8, 16));
        assert_close(pixel(&image, 4, 3), RED);
        assert_close(pixel(&image, 4, 12), BLUE);
    }

    #[test]
    fn decode_jxl_as_bgra() {
        let image = decode(&fixture("red-3x2.jxl")).unwrap();
        assert_eq!((image.width(), image.height()), (3, 2));
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn decode_jxl_with_alpha() {
        let image = decode(&fixture("rgba-2x2.jxl")).unwrap();
        assert_eq!((image.width(), image.height()), (2, 2));
        assert_eq!(pixel(&image, 0, 1), RED);
        assert_eq!(pixel(&image, 1, 1), [255, 0, 0, 128]);
    }

    #[test]
    fn decode_grayscale_jxl() {
        let image = decode(&fixture("gray-2x2.jxl")).unwrap();
        assert_eq!(pixel(&image, 0, 0), [0, 0, 0, 255]);
        assert_eq!(pixel(&image, 1, 0), [255, 255, 255, 255]);
    }

    #[test]
    fn decode_detects_jxl_from_contents() {
        let image = decode(&fixture("jxl-named.png")).unwrap();
        assert_eq!(image.into_pixels(), RED.repeat(6));
    }

    #[test]
    fn decode_fails_on_truncated_jxl() {
        assert!(decode(&fixture("truncated.jxl")).is_err());
    }

    #[test]
    fn decode_fails_on_corrupt_file() {
        assert!(decode(&fixture("truncated.png")).is_err());
    }

    #[test]
    fn decode_fails_on_unsupported_format() {
        assert!(decode(&fixture("README.md")).is_err());
    }

    #[test]
    fn decode_fails_on_missing_file() {
        assert!(decode(&fixture("missing.png")).is_err());
    }

    #[test]
    fn supported_extensions() {
        for name in [
            "a.png", "a.jpg", "a.JPG", "a.Jpeg", "a.webp", "a.gif", "a.jxl", "a.JXL",
        ] {
            assert!(is_supported_image(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn unsupported_extensions() {
        // AVIF and DDS are known to `image` but cannot be read with the
        // enabled features.
        for name in ["a.txt", "a.avif", "a.dds", "png", "a"] {
            assert!(!is_supported_image(Path::new(name)), "{name}");
        }
    }
}
