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

## Animations

Each animated fixture is 4 x 2. The `anim-rgb` ones show red, green and blue
for 100, 200 and 300 ms and loop forever. The `anim-loop2` ones are played
twice and hold short delays, down to 0. `red-3x2.gif` has a single frame.

The GIF and WebP files are made with ImageMagick 7. Its `-loop 2` writes a
GIF repeat count of 1, that is, two plays:

```sh
magick -size 3x2 xc:red -strip red-3x2.gif
magick -loop 0 -delay 10 -size 4x2 xc:red -delay 20 xc:lime -delay 30 xc:blue -strip anim-rgb.gif
magick -loop 2 -delay 0 -size 4x2 xc:red -delay 1 xc:lime -delay 2 xc:blue -strip anim-loop2.gif
magick -loop 0 -delay 10 -size 4x2 xc:red -delay 20 xc:lime -delay 30 xc:blue -strip \
    -define webp:lossless=true anim-rgb.webp
magick -loop 2 -delay 0 -size 4x2 xc:red -delay 1 xc:lime -delay 2 xc:blue -strip \
    -define webp:lossless=true anim-loop2.webp
```

ImageMagick writes a delay of 0 as 100 ms in WebP, and APNG chunks are
easier to control by hand, so the rest is done with Python 3.
`anim-thumbnail.apng` has a white default image that is not part of the
animation, then red with a delay of 0/0 and blue for 25/100 seconds, played
twice. `anim-16bit.apng` has 16 bits per sample, which `image` cannot
composite. `anim-p3-orientation-6.apng` has the Display P3 profile of
`p3-red-8x8.png` and EXIF Orientation = 6:

```python
import struct, zlib

def chunk(kind, data):
    return struct.pack('>I', len(data)) + kind + data + struct.pack('>I', zlib.crc32(kind + data))

def solid(color):
    return lambda x, y: color

def pixels(pixel, width, height, depth):
    def sample(v):
        return bytes([v]) if depth == 8 else struct.pack('>H', v * 257)
    rows = (b'\x00' + b''.join(sample(v) for x in range(width) for v in pixel(x, y))
            for y in range(height))
    return zlib.compress(b''.join(rows))

def apng(name, width, height, frames, num_plays, thumbnail=None, depth=8, chunks=b''):
    """frames: (pixel, (delay_num, delay_den)) each, where pixel(x, y) is an
    RGB color. With `thumbnail`, the default image is that color and not part
    of the animation. `chunks` go before the image data."""
    seq = 0
    def fctl(delay):
        nonlocal seq
        data = struct.pack('>IIIIIHHBB', seq, width, height, 0, 0, *delay, 0, 0)
        seq += 1
        return chunk(b'fcTL', data)
    out = b'\x89PNG\r\n\x1a\n'
    out += chunk(b'IHDR', struct.pack('>IIBBBBB', width, height, depth, 2, 0, 0, 0))
    out += chunks
    out += chunk(b'acTL', struct.pack('>II', len(frames), num_plays))
    if thumbnail is None:
        pixel, delay = frames[0]
        out += fctl(delay) + chunk(b'IDAT', pixels(pixel, width, height, depth))
        rest = frames[1:]
    else:
        out += chunk(b'IDAT', pixels(solid(thumbnail), width, height, depth))
        rest = frames
    for pixel, delay in rest:
        out += fctl(delay)
        out += chunk(b'fdAT', struct.pack('>I', seq) + pixels(pixel, width, height, depth))
        seq += 1
    out += chunk(b'IEND', b'')
    open(name, 'wb').write(out)

def find_chunk(data, kind):
    """The whole chunk of `kind` in a PNG file."""
    i = 8
    while True:
        length = struct.unpack('>I', data[i:i + 4])[0]
        if data[i + 4:i + 8] == kind:
            return data[i:i + 12 + length]
        i += 12 + length

RED, GREEN, BLUE, WHITE = (255, 0, 0), (0, 255, 0), (0, 0, 255), (255, 255, 255)
apng('anim-rgb.apng', 4, 2, [(solid(RED), (1, 10)), (solid(GREEN), (2, 10)), (solid(BLUE), (3, 10))], 0)
# A delay of 0/0 is 0 seconds: a denominator of 0 means 100.
apng('anim-thumbnail.apng', 4, 2, [(solid(RED), (0, 0)), (solid(BLUE), (25, 100))], 2, thumbnail=WHITE)
apng('anim-16bit.apng', 4, 2, [(solid(RED), (1, 10)), (solid(BLUE), (1, 10))], 0, depth=16)

# The Display P3 profile of p3-red-8x8.png and EXIF Orientation = 6. The
# first frame has the red of sRGB in Display P3 on the left and white on the
# right, and the second the other way around.
icc = find_chunk(open('p3-red-8x8.png', 'rb').read(), b'iCCP')
tiff = (b'MM\x00\x2a' + struct.pack('>I', 8) + struct.pack('>H', 1)
        + struct.pack('>HHIHH', 0x0112, 3, 1, 6, 0) + struct.pack('>I', 0))
P3_RED = (234, 51, 35)
apng('anim-p3-orientation-6.apng', 4, 2, [
    (lambda x, y: P3_RED if x < 2 else WHITE, (1, 10)),
    (lambda x, y: WHITE if x < 2 else P3_RED, (1, 10)),
], 0, chunks=icc + chunk(b'eXIf', tiff))

# The duration of the first frame of anim-loop2.webp becomes 0.
data = bytearray(open('anim-loop2.webp', 'rb').read())
duration = data.index(b'ANMF') + 8 + 12
data[duration:duration + 3] = b'\x00\x00\x00'
open('anim-loop2.webp', 'wb').write(data)
```

The animated JPEG XL files are lossless, made with libjxl 0.12 through
`jpegxl-sys` 0.13 with its `vendored` feature, as the safe API of `jpegxl-rs`
cannot set the crop, blending and reference slot of a frame:

- `anim-rgb.jxl`: 10 ticks per second, given as 30 / 3; frames of 1, 2 and 3
  ticks; loops forever. No frame is kept as a reference.
- `anim-loop2.jxl`: 1000 ticks per second; red for 50 ticks, then blue for 0
  ticks as the last frame; played twice.
- `anim-blend.jxl`: 1000 ticks per second; frames of 100 ticks; loops
  forever. Red over the whole image, saved as reference 1; green over the
  right half (crop at x = 2, 2 x 2), replacing reference 1 and saved as it;
  blue over the left half (crop at x = 0, 2 x 2), replacing reference 1.

```rust
use std::{mem::MaybeUninit, ptr::null};

use jpegxl_sys::{
    color::color_encoding::JxlColorEncoding,
    common::types::{JxlBool, JxlDataType, JxlEndianness, JxlPixelFormat},
    encoder::encode::*,
    metadata::codestream_header::{JxlBasicInfo, JxlBlendMode, JxlFrameHeader},
};

struct Frame {
    color: [u8; 3],
    /// x0, y0, width, height
    crop: Option<(i32, i32, u32, u32)>,
    duration: u32,
    source: u32,
    save_as_reference: u32,
}

fn check(status: JxlEncoderStatus) {
    assert!(matches!(status, JxlEncoderStatus::Success));
}

fn encode(width: u32, height: u32, tps: (u32, u32), num_loops: u32, frames: &[Frame]) -> Vec<u8> {
    unsafe {
        let enc = JxlEncoderCreate(null());
        check(JxlEncoderUseContainer(enc, JxlBool::False));
        let mut info = MaybeUninit::<JxlBasicInfo>::uninit();
        JxlEncoderInitBasicInfo(info.as_mut_ptr());
        let mut info = info.assume_init();
        info.xsize = width;
        info.ysize = height;
        info.bits_per_sample = 8;
        info.num_color_channels = 3;
        info.uses_original_profile = JxlBool::True;
        info.have_animation = JxlBool::True;
        info.animation.tps_numerator = tps.0;
        info.animation.tps_denominator = tps.1;
        info.animation.num_loops = num_loops;
        check(JxlEncoderSetBasicInfo(enc, &info));
        let mut color = MaybeUninit::<JxlColorEncoding>::uninit();
        JxlColorEncodingSetToSRGB(color.as_mut_ptr(), JxlBool::False);
        check(JxlEncoderSetColorEncoding(enc, color.as_ptr()));

        for frame in frames {
            let settings = JxlEncoderFrameSettingsCreate(enc, null());
            check(JxlEncoderSetFrameLossless(settings, JxlBool::True));
            let mut header = MaybeUninit::<JxlFrameHeader>::uninit();
            JxlEncoderInitFrameHeader(header.as_mut_ptr());
            let mut header = header.assume_init();
            header.duration = frame.duration;
            let (w, h) = match frame.crop {
                Some((x0, y0, w, h)) => {
                    header.layer_info.have_crop = JxlBool::True;
                    header.layer_info.crop_x0 = x0;
                    header.layer_info.crop_y0 = y0;
                    header.layer_info.xsize = w;
                    header.layer_info.ysize = h;
                    (w, h)
                }
                None => (width, height),
            };
            header.layer_info.blend_info.blendmode = JxlBlendMode::Replace;
            header.layer_info.blend_info.source = frame.source;
            header.layer_info.save_as_reference = frame.save_as_reference;
            check(JxlEncoderSetFrameHeader(settings, &header));
            let pixels = frame.color.repeat((w * h) as usize);
            let format = JxlPixelFormat {
                num_channels: 3,
                data_type: JxlDataType::Uint8,
                endianness: JxlEndianness::Native,
                align: 0,
            };
            check(JxlEncoderAddImageFrame(settings, &format, pixels.as_ptr().cast(), pixels.len()));
        }
        JxlEncoderCloseInput(enc);

        let mut out = vec![0u8; 1 << 16];
        let mut next = out.as_mut_ptr();
        let mut avail = out.len();
        check(JxlEncoderProcessOutput(enc, &mut next, &mut avail));
        out.truncate(out.len() - avail);
        JxlEncoderDestroy(enc);
        out
    }
}

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
let full = |color, duration| Frame { color, crop: None, duration, source: 0, save_as_reference: 0 };
let half = |color, x0, save_as_reference| Frame {
    color,
    crop: Some((x0, 0, 2, 2)),
    duration: 100,
    source: 1,
    save_as_reference,
};
let first = Frame { save_as_reference: 1, ..full(RED, 100) };
std::fs::write("anim-rgb.jxl", encode(4, 2, (30, 3), 0, &[full(RED, 1), full(GREEN, 2), full(BLUE, 3)])).unwrap();
std::fs::write("anim-loop2.jxl", encode(4, 2, (1000, 1), 2, &[full(RED, 50), full(BLUE, 0)])).unwrap();
std::fs::write("anim-blend.jxl", encode(4, 2, (1000, 1), 0, &[first, half(GREEN, 2, 1), half(BLUE, 0, 0)])).unwrap();
```
