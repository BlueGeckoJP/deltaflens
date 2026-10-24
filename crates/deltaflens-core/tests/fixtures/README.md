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
