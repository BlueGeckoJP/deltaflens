//! Animated images, decoded a frame at a time so that only the frames being
//! shown are in memory.

mod gif;
mod webp;

use std::{
    fs::File,
    io::{BufReader, Read},
    num::NonZeroU32,
    path::{Path, PathBuf},
    time::Duration,
};

use eyre::{Result, WrapErr, bail, ensure, eyre};
use image::{
    AnimationDecoder, DynamicImage, Frame, Frames, ImageDecoder, ImageFormat,
    codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder},
    metadata::Orientation,
};

use super::{DecodedImage, convert_to_srgb, jxl, limits, rgba_to_image};

/// How many times an animation is played.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopCount {
    /// Over and over.
    Infinite,
    /// This many times in all, after which it stops on the last frame.
    Finite(NonZeroU32),
}

/// What is known about an animation before its frames are decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimationInfo {
    /// The width of every frame, with the orientation applied.
    pub width: u32,
    /// The height of every frame, with the orientation applied.
    pub height: u32,
    /// The number of frames, at least 2.
    pub frame_count: usize,
    pub loop_count: LoopCount,
}

/// A frame of an animation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// The place of the frame in the animation, from 0.
    pub index: usize,
    /// The whole image at this frame, as [`decode`](super::decode) gives it:
    /// in sRGB, with the orientation applied.
    pub image: DecodedImage,
    /// How long the frame is shown, with delays too short to be meant
    /// replaced as browsers do.
    pub delay: Duration,
}

/// The frames of an animation, decoded one at a time in order.
///
/// It is not `Send`, as the animation decoders of `image` are not: open it on
/// the thread that decodes its frames.
pub trait FrameSource {
    fn info(&self) -> AnimationInfo;

    /// Decodes the next frame. After the last frame, returns `Ok(None)` until
    /// [`seek`](Self::seek) is called, such as with 0 to play it again.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame cannot be read or is corrupt.
    fn next_frame(&mut self) -> Result<Option<DecodedFrame>>;

    /// Makes the frame at `frame_index` the one [`next_frame`] gives next.
    /// Seeking backward may decode the frames from the first again.
    ///
    /// # Errors
    ///
    /// Returns an error if `frame_index` is not less than the number of
    /// frames, or the frames before it cannot be decoded.
    ///
    /// [`next_frame`]: Self::next_frame
    fn seek(&mut self, frame_index: usize) -> Result<()>;
}

/// Opens the animated image at `path` to decode its frames one at a time,
/// detecting the format from the file contents rather than the extension.
///
/// Returns `Ok(None)` for an image of a single frame, or of a format without
/// animations, without decoding its pixels. JPEG XL, GIF, WebP and APNG
/// images can be animated.
///
/// # Errors
///
/// Returns an error if the file cannot be read, or its data is corrupt.
pub fn open_animation(path: &Path) -> Result<Option<Box<dyn FrameSource>>> {
    open_inner(path).wrap_err_with(|| format!("failed to open {}", path.display()))
}

fn open_inner(path: &Path) -> Result<Option<Box<dyn FrameSource>>> {
    let mut header = Vec::with_capacity(jxl::SIGNATURE_LEN);
    File::open(path)?
        .take(jxl::SIGNATURE_LEN as u64)
        .read_to_end(&mut header)?;
    if jxl::has_signature(&header) {
        return Ok(
            jxl::open_animation(path)?.map(|frames| Box::new(frames) as Box<dyn FrameSource>)
        );
    }
    let format = match image::guess_format(&header) {
        Ok(ImageFormat::Gif) => Format::Gif,
        Ok(ImageFormat::Png) => Format::Png,
        Ok(ImageFormat::WebP) => Format::WebP,
        _ => return Ok(None),
    };
    let reader = BufReader::new(File::open(path)?);
    let counted = match format {
        Format::Gif => Some(gif::read_header(reader)?),
        Format::Png => {
            let reader = png::Decoder::new(reader).read_info()?;
            let info = reader.info();
            // `image` composites APNG frames in 8 bits only, and fails on
            // 16-bit ones, so they are shown as still images.
            if info.bit_depth == png::BitDepth::Sixteen {
                return Ok(None);
            }
            info.animation_control().map(|control| Counted {
                frame_count: control.num_frames as usize,
                loop_count: plays(control.num_plays),
            })
        }
        Format::WebP => webp::read_header(reader)?,
    };
    match counted {
        Some(counted) if counted.frame_count >= 2 => {
            Ok(Some(Box::new(ImageFrames::open(path, format, counted)?)))
        }
        _ => Ok(None),
    }
}

/// The frame count and loop count read from the headers of an animation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Counted {
    frame_count: usize,
    loop_count: LoopCount,
}

/// The loop count of a number of plays where 0 means forever, as APNG, WebP
/// and JPEG XL store it.
pub(super) fn plays(count: u32) -> LoopCount {
    NonZeroU32::new(count).map_or(LoopCount::Infinite, LoopCount::Finite)
}

/// What a delay too short to be meant becomes, as in browsers: GIF delays of
/// up to 10 ms and delays of 0 in the other formats. This keeps such
/// animations from running at full speed.
const DEFAULT_DELAY: Duration = Duration::from_millis(100);

/// The longest delay of a GIF frame taken as not meant.
const GIF_TOO_SHORT: Duration = Duration::from_millis(10);

/// Returns `delay`, or [`DEFAULT_DELAY`] if it is no longer than
/// `too_short`.
pub(super) fn normalize_delay(delay: Duration, too_short: Duration) -> Duration {
    if delay <= too_short {
        DEFAULT_DELAY
    } else {
        delay
    }
}

/// The animated formats that `image` decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Gif,
    Png,
    WebP,
}

impl Format {
    fn too_short_delay(self) -> Duration {
        match self {
            Format::Gif => GIF_TOO_SHORT,
            Format::Png | Format::WebP => Duration::ZERO,
        }
    }
}

/// The frames of a GIF, APNG or WebP animation, through the animation
/// decoders of `image`, which have no random access: seeking backward
/// decodes from the first frame again.
struct ImageFrames {
    path: PathBuf,
    format: Format,
    info: AnimationInfo,
    /// `None` after a frame fails to decode, as the decoders give no more
    /// frames then, until a seek opens the file again.
    frames: Option<Frames<'static>>,
    orientation: Orientation,
    /// The color space to convert from, if it is given by an ICC profile.
    icc: Option<Vec<u8>>,
    /// The index of the frame `frames` gives next.
    next: usize,
}

impl ImageFrames {
    fn open(path: &Path, format: Format, counted: Counted) -> Result<Self> {
        let Opened {
            frames,
            width,
            height,
            orientation,
            icc,
        } = open_frames(path, format)?;
        let icc = usable_profile(icc, path);
        let (width, height) = if swaps_axes(orientation) {
            (height, width)
        } else {
            (width, height)
        };
        Ok(Self {
            path: path.to_path_buf(),
            format,
            info: AnimationInfo {
                width,
                height,
                frame_count: counted.frame_count,
                loop_count: counted.loop_count,
            },
            frames: Some(frames),
            orientation,
            icc,
            next: 0,
        })
    }

    /// Reads the next frame without converting it.
    fn read(&mut self) -> Result<Frame> {
        let Some(frames) = &mut self.frames else {
            bail!("a frame failed to decode before");
        };
        let result = match frames.next() {
            Some(frame) => frame.map_err(Into::into),
            None => Err(eyre!(
                "the animation ends after {} of {} frames",
                self.next,
                self.info.frame_count
            )),
        };
        if result.is_err() {
            self.frames = None;
        }
        result
    }
}

impl FrameSource for ImageFrames {
    fn info(&self) -> AnimationInfo {
        self.info
    }

    fn next_frame(&mut self) -> Result<Option<DecodedFrame>> {
        let index = self.next;
        if index == self.info.frame_count {
            return Ok(None);
        }
        let frame = self
            .read()
            .wrap_err_with(|| format!("failed to decode frame {index}"))?;
        self.next += 1;
        let delay = normalize_delay(frame.delay().into(), self.format.too_short_delay());
        let mut image = DynamicImage::ImageRgba8(frame.into_buffer());
        image.apply_orientation(self.orientation);
        let image = rgba_to_image(image.into_rgba8(), self.icc.as_deref(), &self.path);
        Ok(Some(DecodedFrame {
            index,
            image,
            delay,
        }))
    }

    fn seek(&mut self, frame_index: usize) -> Result<()> {
        ensure!(
            frame_index < self.info.frame_count,
            "frame {frame_index} is out of {} frames",
            self.info.frame_count
        );
        if self.frames.is_none() || frame_index < self.next {
            // Dropped first, so that none are left if opening fails.
            self.frames = None;
            self.frames = Some(open_frames(&self.path, self.format)?.frames);
            self.next = 0;
        }
        while self.next < frame_index {
            self.read()
                .wrap_err_with(|| format!("failed to decode frame {}", self.next))?;
            self.next += 1;
        }
        Ok(())
    }
}

/// An animation decoder of `image` turned into its frames.
struct Opened {
    frames: Frames<'static>,
    width: u32,
    height: u32,
    orientation: Orientation,
    icc: Option<Vec<u8>>,
}

/// Opens the frames of the animation at `path` from its first frame.
fn open_frames(path: &Path, format: Format) -> Result<Opened> {
    let reader = BufReader::new(File::open(path)?);
    match format {
        Format::Gif => {
            let mut decoder = GifDecoder::new(reader)?;
            decoder.set_limits(limits())?;
            Opened::new(decoder, |decoder| Ok(decoder.into_frames()), path)
        }
        Format::Png => {
            let decoder = PngDecoder::with_limits(reader, limits())?;
            Opened::new(decoder, |decoder| Ok(decoder.apng()?.into_frames()), path)
        }
        Format::WebP => {
            let mut decoder = WebPDecoder::new(reader)?;
            decoder.set_limits(limits())?;
            Opened::new(decoder, |decoder| Ok(decoder.into_frames()), path)
        }
    }
}

impl Opened {
    fn new<D: ImageDecoder>(
        mut decoder: D,
        into_frames: impl FnOnce(D) -> Result<Frames<'static>>,
        path: &Path,
    ) -> Result<Self> {
        let (width, height) = decoder.dimensions();
        // The decoders allocate a canvas of this size, and the WebP one does
        // not check the limits.
        limits().reserve(u64::from(width) * u64::from(height) * 4)?;
        let orientation = decoder.orientation()?;
        let icc = decoder.icc_profile().unwrap_or_else(|error| {
            tracing::warn!(
                "{}: failed to read the ICC profile: {error}",
                path.display()
            );
            None
        });
        Ok(Self {
            frames: into_frames(decoder)?,
            width,
            height,
            orientation,
            icc,
        })
    }
}

/// Returns whether `orientation` turns the image by 90 or 270 degrees.
fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Returns the ICC profile `icc` if colors can be converted from it to sRGB,
/// or else `None` after a warning. Checked once for an animation, so that a
/// profile that cannot be used does not warn at every frame.
pub(super) fn usable_profile(icc: Option<Vec<u8>>, path: &Path) -> Option<Vec<u8>> {
    let icc = icc?;
    match convert_to_srgb(&mut [0, 0, 0, u8::MAX], &icc) {
        Ok(()) => Some(icc),
        Err(error) => {
            tracing::warn!(
                "{}: colors are not converted to sRGB: {error:#}",
                path.display()
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::{decode, test_util::TempDir};

    const RED: [u8; 4] = [0, 0, 255, 255];
    const GREEN: [u8; 4] = [0, 255, 0, 255];
    const BLUE: [u8; 4] = [255, 0, 0, 255];

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn open(name: &str) -> Box<dyn FrameSource> {
        open_animation(&fixture(name)).unwrap().unwrap()
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    fn finite(plays: u32) -> LoopCount {
        LoopCount::Finite(NonZeroU32::new(plays).unwrap())
    }

    fn pixel(image: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        image.pixels().as_chunks::<4>().0[(y * image.width() + x) as usize]
    }

    /// The color of a frame that has one color all over.
    fn color(frame: &DecodedFrame) -> [u8; 4] {
        let first = pixel(&frame.image, 0, 0);
        assert!(
            frame
                .image
                .pixels()
                .as_chunks::<4>()
                .0
                .iter()
                .all(|&p| p == first),
            "frame {} has more than one color",
            frame.index
        );
        first
    }

    /// Asserts that the colors are the same, but for 1 in each channel:
    /// image-webp 0.2 blends an opaque pixel over another to 254 for 255.
    #[track_caller]
    fn assert_colors(actual: &[[u8; 4]], expected: &[[u8; 4]], name: &str) {
        let close = actual.len() == expected.len()
            && actual.iter().zip(expected).all(|(actual, expected)| {
                actual
                    .iter()
                    .zip(expected)
                    .all(|(&a, &e)| a.abs_diff(e) <= 1)
            });
        assert!(close, "{name}: {actual:?} is not close to {expected:?}");
    }

    /// Reads the frames up to the end, which must be repeated.
    fn read_to_end(source: &mut dyn FrameSource) -> Vec<DecodedFrame> {
        let mut frames = Vec::new();
        while let Some(frame) = source.next_frame().unwrap() {
            assert_eq!(frame.index, frames.len());
            let info = source.info();
            assert_eq!(
                (frame.image.width(), frame.image.height()),
                (info.width, info.height)
            );
            frames.push(frame);
        }
        assert_eq!(source.next_frame().unwrap(), None);
        frames
    }

    const RGB: [&str; 4] = [
        "anim-rgb.gif",
        "anim-rgb.webp",
        "anim-rgb.apng",
        "anim-rgb.jxl",
    ];

    #[test]
    fn single_frame_images_are_not_animations() {
        for name in [
            "red-3x2.png",
            "red-3x2.jpg",
            "red-3x2.webp",
            "red-3x2.jxl",
            "red-3x2.gif",
            "orientation-6.jxl",
            "README.md",
        ] {
            assert!(open_animation(&fixture(name)).unwrap().is_none(), "{name}");
        }
    }

    #[test]
    fn open_fails_on_missing_file() {
        assert!(open_animation(&fixture("missing.gif")).is_err());
    }

    #[test]
    fn reads_the_info() {
        for name in RGB {
            let info = open(name).info();
            let expected = AnimationInfo {
                width: 4,
                height: 2,
                frame_count: 3,
                loop_count: LoopCount::Infinite,
            };
            assert_eq!(info, expected, "{name}");
        }
    }

    #[test]
    fn decodes_each_frame_with_its_delay() {
        for name in RGB {
            let frames = read_to_end(&mut *open(name));
            let colors: Vec<_> = frames.iter().map(color).collect();
            assert_colors(&colors, &[RED, GREEN, BLUE], name);
            let delays: Vec<_> = frames.iter().map(|frame| frame.delay).collect();
            assert_eq!(delays, [ms(100), ms(200), ms(300)], "{name}");
        }
    }

    #[test]
    fn seeks_forward() {
        for name in RGB {
            let mut source = open(name);
            source.seek(2).unwrap();
            let frame = source.next_frame().unwrap().unwrap();
            assert_eq!(frame.index, 2, "{name}");
            assert_colors(&[color(&frame)], &[BLUE], name);
            assert_eq!(source.next_frame().unwrap(), None, "{name}");
        }
    }

    #[test]
    fn seeks_backward() {
        for name in RGB {
            let mut source = open(name);
            read_to_end(&mut *source);
            source.seek(1).unwrap();
            let frame = source.next_frame().unwrap().unwrap();
            assert_eq!(frame.index, 1, "{name}");
            assert_colors(&[color(&frame)], &[GREEN], name);
            // To the same place again.
            source.seek(2).unwrap();
            source.seek(2).unwrap();
            let frame = source.next_frame().unwrap().unwrap();
            assert_eq!(frame.index, 2, "{name}");
            assert_colors(&[color(&frame)], &[BLUE], name);
        }
    }

    #[test]
    fn plays_again_after_seeking_to_the_first_frame() {
        for name in RGB {
            let mut source = open(name);
            let first = read_to_end(&mut *source);
            source.seek(0).unwrap();
            assert_eq!(read_to_end(&mut *source), first, "{name}");
        }
    }

    #[test]
    fn seek_beyond_the_last_frame_fails() {
        for name in RGB {
            let mut source = open(name);
            assert!(source.seek(3).is_err(), "{name}");
            // The position is kept.
            let frame = source.next_frame().unwrap().unwrap();
            assert_eq!(frame.index, 0, "{name}");
        }
    }

    #[test]
    fn finite_loops_and_too_short_delays() {
        // GIF: delays of 0, 10 and 20 ms; repeated once.
        let mut source = open("anim-loop2.gif");
        assert_eq!(source.info().loop_count, finite(2));
        let delays: Vec<_> = read_to_end(&mut *source).iter().map(|f| f.delay).collect();
        assert_eq!(delays, [ms(100), ms(100), ms(20)]);

        // WebP: delays of 0, 10 and 20 ms; played twice.
        let mut source = open("anim-loop2.webp");
        assert_eq!(source.info().loop_count, finite(2));
        let delays: Vec<_> = read_to_end(&mut *source).iter().map(|f| f.delay).collect();
        assert_eq!(delays, [ms(100), ms(10), ms(20)]);

        // JPEG XL: durations of 50 ms and 0; played twice.
        let mut source = open("anim-loop2.jxl");
        assert_eq!(source.info().loop_count, finite(2));
        let frames = read_to_end(&mut *source);
        let colors: Vec<_> = frames.iter().map(color).collect();
        assert_eq!(colors, [RED, BLUE]);
        let delays: Vec<_> = frames.iter().map(|f| f.delay).collect();
        assert_eq!(delays, [ms(50), ms(100)]);
    }

    #[test]
    fn apng_default_image_outside_the_animation_is_skipped() {
        // The default image is white and not a frame; then red with a delay
        // of 0/0 and blue for 25/100 seconds, played twice.
        let mut source = open("anim-thumbnail.apng");
        let info = source.info();
        assert_eq!((info.frame_count, info.loop_count), (2, finite(2)));
        let frames = read_to_end(&mut *source);
        let colors: Vec<_> = frames.iter().map(color).collect();
        assert_eq!(colors, [RED, BLUE]);
        let delays: Vec<_> = frames.iter().map(|f| f.delay).collect();
        assert_eq!(delays, [ms(100), ms(250)]);
    }

    #[test]
    fn jxl_frames_drawn_over_earlier_ones() {
        // Red, then green over the right half, then blue over the left half.
        let mut source = open("anim-blend.jxl");
        let halves = |frame: &DecodedFrame| (pixel(&frame.image, 0, 0), pixel(&frame.image, 3, 1));
        let frames = read_to_end(&mut *source);
        let expected = [(RED, RED), (RED, GREEN), (BLUE, GREEN)];
        assert_eq!(frames.iter().map(halves).collect::<Vec<_>>(), expected);

        source.seek(1).unwrap();
        let frame = source.next_frame().unwrap().unwrap();
        assert_eq!(halves(&frame), expected[1]);
        source.seek(0).unwrap();
        assert_eq!(read_to_end(&mut *source), frames);
    }

    #[test]
    fn applies_the_orientation_and_the_icc_profile_to_each_frame() {
        // Stored as 4 x 2 with EXIF Orientation = 6 and a Display P3
        // profile: the red of sRGB on the left and white on the right, then
        // the other way around. Turned 90° clockwise, the left is on top.
        let mut source = open("anim-p3-orientation-6.apng");
        let info = source.info();
        assert_eq!((info.width, info.height), (2, 4));
        let frames = read_to_end(&mut *source);
        let ends = |frame: &DecodedFrame| [pixel(&frame.image, 1, 0), pixel(&frame.image, 0, 3)];
        let close = |actual: [[u8; 4]; 2], expected: [[u8; 4]; 2]| {
            actual
                .iter()
                .flatten()
                .zip(expected.iter().flatten())
                .all(|(&a, &e)| a.abs_diff(e) <= 8)
        };
        let white = [255; 4];
        assert!(
            close(ends(&frames[0]), [RED, white]),
            "{:?}",
            ends(&frames[0])
        );
        assert!(
            close(ends(&frames[1]), [white, RED]),
            "{:?}",
            ends(&frames[1])
        );
    }

    #[test]
    fn sixteen_bit_apng_is_a_still_image() {
        let path = fixture("anim-16bit.apng");
        assert!(open_animation(&path).unwrap().is_none());
        assert_eq!(decode(&path).unwrap().into_pixels(), RED.repeat(8));
    }

    #[test]
    fn frames_after_a_failed_one_need_a_seek() {
        // An invalid LZW code size in the second frame.
        let mut data = fs::read(fixture("anim-rgb.gif")).unwrap();
        let descriptor = b"\x2c\x00\x00\x00\x00\x04\x00\x02\x00\x80";
        let start = data
            .windows(descriptor.len())
            .position(|window| window == descriptor)
            .unwrap();
        // After the descriptor and a local color table of 2 colors.
        data[start + descriptor.len() + 6] = 0x0f;
        let dir = TempDir::with_files("animation-failed-frame", &[]);
        let path = dir.path().join("anim.gif");
        fs::write(&path, data).unwrap();

        let mut source = open_animation(&path).unwrap().unwrap();
        assert_eq!(source.next_frame().unwrap().unwrap().index, 0);
        assert!(source.next_frame().is_err());
        assert!(source.next_frame().is_err());
        source.seek(0).unwrap();
        assert_eq!(color(&source.next_frame().unwrap().unwrap()), RED);
        assert!(source.seek(2).is_err());
        source.seek(0).unwrap();
        assert_eq!(color(&source.next_frame().unwrap().unwrap()), RED);
    }

    #[test]
    fn detects_the_format_from_the_contents() {
        let dir = TempDir::with_files("animation-format", &[]);
        let path = dir.path().join("anim.jpg");
        fs::copy(fixture("anim-rgb.gif"), &path).unwrap();
        let mut source = open_animation(&path).unwrap().unwrap();
        assert_eq!(read_to_end(&mut *source).len(), 3);
    }

    #[test]
    fn decode_gives_the_first_frame() {
        for name in RGB {
            let image = decode(&fixture(name)).unwrap();
            assert_eq!((image.width(), image.height()), (4, 2), "{name}");
            assert_eq!(image.into_pixels(), RED.repeat(8), "{name}");
        }
        // The default image of an APNG, even if it is not a frame.
        let image = decode(&fixture("anim-thumbnail.apng")).unwrap();
        assert_eq!(image.into_pixels(), [255; 4].repeat(8));
    }

    #[test]
    fn gif_delays_up_to_10_ms_are_too_short() {
        for (delay, expected) in [(0, 100), (10, 100), (11, 11), (20, 20)] {
            assert_eq!(normalize_delay(ms(delay), GIF_TOO_SHORT), ms(expected));
        }
    }

    #[test]
    fn other_delays_of_0_are_too_short() {
        for format in [Format::Png, Format::WebP] {
            for (delay, expected) in [(0, 100), (1, 1), (10, 10)] {
                assert_eq!(
                    normalize_delay(ms(delay), format.too_short_delay()),
                    ms(expected)
                );
            }
        }
    }

    #[test]
    fn profiles_that_cannot_convert_colors_are_dropped() {
        let path = fixture("anim-p3-orientation-6.apng");
        let mut decoder = PngDecoder::new(BufReader::new(File::open(&path).unwrap())).unwrap();
        let p3 = decoder.icc_profile().unwrap().unwrap();
        assert_eq!(usable_profile(Some(p3.clone()), &path), Some(p3));
        assert_eq!(usable_profile(Some(b"not a profile".to_vec()), &path), None);
        assert_eq!(usable_profile(None, &path), None);
    }
}
