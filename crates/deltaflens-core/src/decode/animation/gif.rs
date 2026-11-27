//! Counting the frames of a GIF from its blocks, without decompressing them.

use std::{
    io::{self, ErrorKind, Read},
    num::NonZeroU32,
};

use eyre::{Result, ensure};

use super::{Counted, LoopCount};

/// The label of an Application Extension.
const APPLICATION_LABEL: u8 = 0xff;
/// Application Extensions that tell how many times to repeat an animation.
/// Their first sub-block is 1 and the repeat count in 16-bit little endian.
const LOOP_EXTENSIONS: [&[u8]; 2] = [b"NETSCAPE2.0", b"ANIMEXTS1.0"];

/// Reads the frame count and loop count of a GIF.
///
/// Reading stops at the trailer, at the end of the file, or at an unknown
/// block, after which `image` decodes no frames either. A frame cut off by
/// the end of the file is not counted.
///
/// Without a loop extension the animation is played once, and a repeat count
/// of `n` plays it `n + 1` times, or forever if `n` is 0, as in browsers.
pub(super) fn read_header(mut reader: impl Read) -> Result<Counted> {
    let mut header = [0; 13];
    reader.read_exact(&mut header)?;
    ensure!(
        header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a"),
        "not a GIF"
    );
    // The Logical Screen Descriptor follows the signature.
    skip_color_table(&mut reader, header[10])?;

    let mut frame_count = 0;
    let mut repeat = None;
    match read_blocks(&mut reader, &mut frame_count, &mut repeat) {
        Err(error) if error.kind() != ErrorKind::UnexpectedEof => return Err(error.into()),
        _ => {}
    }
    let loop_count = match repeat {
        None => LoopCount::Finite(NonZeroU32::MIN),
        Some(0) => LoopCount::Infinite,
        Some(n) => LoopCount::Finite(NonZeroU32::MIN.saturating_add(u32::from(n))),
    };
    Ok(Counted {
        frame_count,
        loop_count,
    })
}

/// Reads the blocks after the Logical Screen Descriptor, counting the image
/// descriptors and keeping the repeat count of the last loop extension.
fn read_blocks(
    reader: &mut impl Read,
    frame_count: &mut usize,
    repeat: &mut Option<u16>,
) -> io::Result<()> {
    loop {
        match read_u8(reader)? {
            // Extension
            0x21 => {
                let label = read_u8(reader)?;
                if label != APPLICATION_LABEL {
                    read_sub_blocks(reader, |_| {})?;
                    continue;
                }
                // The identifier is the first sub-block, and an empty one
                // ends the extension.
                let mut identifier = [0; 255];
                let identifier = &mut identifier[..usize::from(read_u8(reader)?)];
                if identifier.is_empty() {
                    continue;
                }
                reader.read_exact(identifier)?;
                let is_loop = LOOP_EXTENSIONS.contains(&&*identifier);
                read_sub_blocks(reader, |data| {
                    if is_loop && let [1, low, high, ..] = *data {
                        *repeat = Some(u16::from_le_bytes([low, high]));
                    }
                })?;
            }
            // Image Descriptor
            0x2c => {
                let mut descriptor = [0; 9];
                reader.read_exact(&mut descriptor)?;
                skip_color_table(reader, descriptor[8])?;
                // The minimum code size of LZW, then the image data.
                read_u8(reader)?;
                read_sub_blocks(reader, |_| {})?;
                *frame_count += 1;
            }
            // Trailer, or an unknown block
            _ => return Ok(()),
        }
    }
}

/// Skips the color table that the packed fields `flags` of a Logical Screen
/// Descriptor or an Image Descriptor tell of, if any.
fn skip_color_table(reader: &mut impl Read, flags: u8) -> io::Result<()> {
    if flags & 0x80 == 0 {
        return Ok(());
    }
    let len = 3 << ((flags & 0x07) + 1);
    let skipped = io::copy(&mut reader.take(len), &mut io::sink())?;
    if skipped < len {
        return Err(ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}

/// Reads data sub-blocks up to the empty one that ends them, giving the data
/// of each to `f`.
fn read_sub_blocks(reader: &mut impl Read, mut f: impl FnMut(&[u8])) -> io::Result<()> {
    let mut buffer = [0; 255];
    loop {
        let data = &mut buffer[..usize::from(read_u8(reader)?)];
        if data.is_empty() {
            return Ok(());
        }
        reader.read_exact(data)?;
        f(data);
    }
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut byte = [0];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read(path).unwrap()
    }

    /// The bytes of the NETSCAPE2.0 extension of `data`, through its
    /// terminator.
    fn loop_extension(data: &[u8]) -> std::ops::Range<usize> {
        let start = data
            .windows(14)
            .position(|window| window == b"\x21\xff\x0bNETSCAPE2.0")
            .unwrap();
        start..start + 19
    }

    #[test]
    fn counts_frames_and_reads_infinite_loop() {
        let counted = read_header(&fixture("anim-rgb.gif")[..]).unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(counted.loop_count, LoopCount::Infinite);
    }

    #[test]
    fn repeat_count_adds_to_the_first_play() {
        // Made with `-loop 2`, which ImageMagick writes as a repeat count of
        // 1.
        let counted = read_header(&fixture("anim-loop2.gif")[..]).unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(
            counted.loop_count,
            LoopCount::Finite(NonZeroU32::new(2).unwrap())
        );
    }

    #[test]
    fn animation_without_loop_extension_plays_once() {
        let mut data = fixture("anim-rgb.gif");
        data.drain(loop_extension(&data));
        let counted = read_header(&data[..]).unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(counted.loop_count, LoopCount::Finite(NonZeroU32::MIN));
    }

    #[test]
    fn reads_animexts_loop_extension() {
        let mut data = fixture("anim-loop2.gif");
        let range = loop_extension(&data);
        data[range.start + 3..range.start + 14].copy_from_slice(b"ANIMEXTS1.0");
        let counted = read_header(&data[..]).unwrap();
        assert_eq!(
            counted.loop_count,
            LoopCount::Finite(NonZeroU32::new(2).unwrap())
        );
    }

    #[test]
    fn ignores_other_application_extensions() {
        let mut data = fixture("anim-loop2.gif");
        let range = loop_extension(&data);
        data[range.start + 3..range.start + 14].copy_from_slice(b"XMP DataXMP");
        let counted = read_header(&data[..]).unwrap();
        assert_eq!(counted.loop_count, LoopCount::Finite(NonZeroU32::MIN));
    }

    #[test]
    fn empty_application_extension_ends_itself() {
        let mut data = fixture("anim-rgb.gif");
        let start = loop_extension(&data).start;
        data.splice(start..start, *b"\x21\xff\x00");
        let counted = read_header(&data[..]).unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(counted.loop_count, LoopCount::Infinite);
    }

    #[test]
    fn counts_a_single_frame() {
        let counted = read_header(&fixture("red-3x2.gif")[..]).unwrap();
        assert_eq!(counted.frame_count, 1);
    }

    #[test]
    fn frame_cut_off_by_the_end_is_not_counted() {
        let data = fixture("anim-rgb.gif");
        // Without the trailer, all frames are whole.
        let counted = read_header(&data[..data.len() - 1]).unwrap();
        assert_eq!(counted.frame_count, 3);
        // Without the end of the last image data.
        let counted = read_header(&data[..data.len() - 3]).unwrap();
        assert_eq!(counted.frame_count, 2);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(read_header(&fixture("red-3x2.png")[..]).is_err());
        assert!(read_header(&b"GIF89a"[..]).is_err());
    }
}
