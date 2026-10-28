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
