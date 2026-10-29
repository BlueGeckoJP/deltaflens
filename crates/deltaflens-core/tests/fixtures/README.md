# Test fixtures

Generated with ImageMagick 7 and Python 3:

```sh
magick -size 3x2 xc:red -strip red-3x2.png
magick -size 3x2 xc:red -strip -define webp:lossless=true red-3x2.webp
magick -size 3x2 xc:red -strip -quality 100 -sampling-factor 1x1 red-3x2.jpg
# A PNG with a misleading extension.
cp red-3x2.png png-named.jpg
head -c 40 red-3x2.png > truncated.png

# 16x8, red on the left and blue on the right, with EXIF Orientation = 6
# ("rotate 90° clockwise to display"). ImageMagick drops the tag without an
# existing EXIF profile, so the APP1 segment is inserted by hand.
magick -size 8x8 xc:red -size 8x8 xc:blue +append -strip -quality 100 -sampling-factor 1x1 plain.jpg
python3 - <<'EOF'
import struct
data = open('plain.jpg', 'rb').read()
tiff = (b'MM\x00\x2a' + struct.pack('>I', 8) + struct.pack('>H', 1)
        + struct.pack('>HHIHH', 0x0112, 3, 1, 6, 0) + struct.pack('>I', 0))
payload = b'Exif\x00\x00' + tiff
app1 = b'\xff\xe1' + struct.pack('>H', len(payload) + 2) + payload
open('orientation-6.jpg', 'wb').write(data[:2] + app1 + data[2:])
EOF
rm plain.jpg
```

The JPEG XL fixtures are lossless, made with the pure-Rust encoder
`zune-jpegxl` 0.4 (no JPEG XL encoder is packaged for the development
machine):

```rust
use zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions};
use zune_jpegxl::JxlSimpleEncoder;

fn write(name: &str, pixels: &[u8], width: usize, height: usize, colorspace: ColorSpace) {
    let options = EncoderOptions::new(width, height, colorspace, BitDepth::Eight);
    let encoded = JxlSimpleEncoder::new(pixels, options).encode().unwrap();
    std::fs::write(name, encoded).unwrap();
}

// The encoder needs a height of at least 2.
write("red-3x2.jxl", &[255, 0, 0].repeat(6), 3, 2, ColorSpace::RGB);
// Each row: opaque red, half-transparent blue.
write("rgba-2x2.jxl", &[255, 0, 0, 255, 0, 0, 255, 128].repeat(2), 2, 2, ColorSpace::RGBA);
// Each row: black, white.
write("gray-2x2.jxl", &[0, 255].repeat(2), 2, 2, ColorSpace::Luma);
```

```sh
# A JPEG XL file with a misleading extension.
cp red-3x2.jxl jxl-named.png
head -c 20 red-3x2.jxl > truncated.jxl
```

The fixtures with a Display P3 profile hold the red of sRGB in Display P3,
(234, 51, 35). They are made with `image` 0.25, `moxcms` 0.8 and `jpegxl-rs`
0.16 (libjxl 0.12, built from source with its `vendored` feature). The JPEG
XL file is the JPEG recompressed losslessly, which keeps its ICC profile:

```rust
use std::io::Cursor;

use image::{ExtendedColorType, ImageEncoder, codecs::{jpeg::JpegEncoder, png::PngEncoder}};

let icc = moxcms::ColorProfile::new_display_p3().encode().unwrap();
let pixels = [234u8, 51, 35].repeat(64);

let mut jpeg = Vec::new();
let mut encoder = JpegEncoder::new_with_quality(Cursor::new(&mut jpeg), 100);
encoder.set_icc_profile(icc.clone()).unwrap();
encoder.write_image(&pixels, 8, 8, ExtendedColorType::Rgb8).unwrap();
std::fs::write("p3-red-8x8.jpg", &jpeg).unwrap();

let jxl = jpegxl_rs::encode::encoder_builder().build().unwrap().encode_jpeg(&jpeg).unwrap();
std::fs::write("p3-red-8x8.jxl", jxl).unwrap();

let mut png = Vec::new();
let mut encoder = PngEncoder::new(Cursor::new(&mut png));
encoder.set_icc_profile(icc).unwrap();
encoder.write_image(&pixels, 8, 8, ExtendedColorType::Rgb8).unwrap();
std::fs::write("p3-red-8x8.png", png).unwrap();
```

`orientation-6.jxl` is `orientation-6.jpg` recompressed losslessly in the
same way, which keeps its orientation. `progressive-256.jxl` is a 256 x 256
image of a gradient with a grid, encoded with `jpegxl-rs` as lossy and
progressive:

```rust
use jpegxl_rs::encode::{EncoderSpeed, encoder_builder};
use jpegxl_sys::encoder::encode::JxlEncoderFrameSettingId;

let mut encoder = encoder_builder()
    .speed(EncoderSpeed::Squirrel)
    .quality(1.0)
    .build()
    .unwrap();
encoder.set_frame_option(JxlEncoderFrameSettingId::ProgressiveDc, 1);
encoder.set_frame_option(JxlEncoderFrameSettingId::ProgressiveAc, 1);
encoder.set_frame_option(JxlEncoderFrameSettingId::Responsive, 1);
let jxl = encoder.encode::<u8>(&rgb_pixels, 256, 256).unwrap();

`p3-table-curve-xyb-8x8.jxl` is lossy, so stored in XYB, with an ICC profile
of Display P3 primaries and tone curves given as 64-entry tables of gamma 2.0.
jxl-oxide cannot write this profile as an enum color encoding, so it renders
the image in sRGB. The pixels are (234, 51, 35) in the profile. The safe API
of `jpegxl-rs` cannot set an ICC profile, so it is made with `jpegxl-sys`:

```rust
let mut profile = moxcms::ColorProfile::new_display_p3();
let table: Vec<u16> = (0..64)
    .map(|i| ((i as f64 / 63.0).powf(2.0) * 65535.0).round() as u16)
    .collect();
let curve = moxcms::ToneReprCurve::Lut(table);
profile.red_trc = Some(curve.clone());
profile.green_trc = Some(curve.clone());
profile.blue_trc = Some(curve);
profile.cicp = None;
let icc = profile.encode().unwrap();

// With jpegxl_sys: JxlEncoderSetBasicInfo with 8 x 8, 8 bits per sample,
// 3 color channels and `uses_original_profile` false; JxlEncoderSetICCProfile
// with `icc`; JxlEncoderSetFrameDistance 0.5; then the 8-bit RGB pixels
// [234, 51, 35] x 64 with JxlEncoderAddImageFrame.
```

`red-3x2.avif` is made with the pure-Rust encoder `ravif` 0.13:

```rust
let pixels = vec![rgb::RGBA8::new(255, 0, 0, 255); 6];
let encoded = ravif::Encoder::new()
    .with_quality(100.0)
    .with_speed(10)
    .encode_rgba(ravif::Img::new(&pixels[..], 3, 2))
    .unwrap();
std::fs::write("red-3x2.avif", encoded.avif_file).unwrap();
```
