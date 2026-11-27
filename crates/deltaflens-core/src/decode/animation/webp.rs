//! Counting the frames of a WebP from its chunk headers, without decoding
//! them.

use std::io::{self, ErrorKind, Read};

use eyre::{Result, ensure};

use super::{Counted, plays};

/// The flag of the VP8X chunk for an animation.
const ANIMATION_FLAG: u8 = 0x02;

/// Reads the frame count and loop count of a WebP, or returns `None` if it is
/// not animated.
///
/// Reading stops at the end of the RIFF chunk or of the file. A frame cut off
/// by the end of the file is not counted.
pub(super) fn read_header(mut reader: impl Read) -> Result<Option<Counted>> {
    let mut header = [0; 12];
    reader.read_exact(&mut header)?;
    ensure!(
        header.starts_with(b"RIFF") && &header[8..] == b"WEBP",
        "not a WebP"
    );
    let riff_size = u64::from(u32::from_le_bytes([
        header[4], header[5], header[6], header[7],
    ]));
    // The size counts "WEBP" and the chunks after it.
    let mut reader = reader.take(riff_size.saturating_sub(4));

    // A simple file has a VP8 or VP8L chunk only, and an extended one starts
    // with VP8X, whose first byte holds the flags.
    let Some((kind, size)) = read_chunk_header(&mut reader)? else {
        return Ok(None);
    };
    if &kind != b"VP8X" || size < 1 {
        return Ok(None);
    }
    let flags = read_u8(&mut reader)?;
    if flags & ANIMATION_FLAG == 0 {
        return Ok(None);
    }
    skip(&mut reader, padded(size) - 1)?;

    let mut frame_count = 0;
    // Set by the ANIM chunk, without which `image` does not open the
    // animation.
    let mut loop_count = plays(0);
    while let Some((kind, size)) = read_chunk_header(&mut reader)? {
        let result = match &kind {
            b"ANIM" if size >= 6 => {
                // The background color, then the loop count.
                let mut data = [0; 6];
                reader
                    .read_exact(&mut data)
                    .and_then(|()| skip(&mut reader, padded(size) - 6))
                    .map(|()| loop_count = plays(u16::from_le_bytes([data[4], data[5]]).into()))
            }
            b"ANMF" => skip(&mut reader, padded(size)).map(|()| frame_count += 1),
            _ => skip(&mut reader, padded(size)),
        };
        match result {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(Some(Counted {
        frame_count,
        loop_count,
    }))
}

/// Reads the `FourCC` and the size of the next chunk, or returns `None` at the
/// end.
fn read_chunk_header(reader: &mut impl Read) -> io::Result<Option<([u8; 4], u64)>> {
    let mut header = [0; 8];
    match reader.read_exact(&mut header) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let kind = [header[0], header[1], header[2], header[3]];
    let size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    Ok(Some((kind, u64::from(size))))
}

/// The size of chunk data of `size` with its padding, as chunks start at even
/// offsets.
fn padded(size: u64) -> u64 {
    size + size % 2
}

fn skip(reader: &mut impl Read, len: u64) -> io::Result<()> {
    let skipped = io::copy(&mut reader.take(len), &mut io::sink())?;
    if skipped < len {
        return Err(ErrorKind::UnexpectedEof.into());
    }
    Ok(())
}

fn read_u8(reader: &mut impl Read) -> io::Result<u8> {
    let mut byte = [0];
    reader.read_exact(&mut byte)?;
    Ok(byte[0])
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use super::*;
    use crate::decode::LoopCount;

    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        std::fs::read(path).unwrap()
    }

    #[test]
    fn counts_frames_and_reads_infinite_loop() {
        let counted = read_header(&fixture("anim-rgb.webp")[..]).unwrap().unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(counted.loop_count, LoopCount::Infinite);
    }

    #[test]
    fn reads_finite_loop() {
        let counted = read_header(&fixture("anim-loop2.webp")[..])
            .unwrap()
            .unwrap();
        assert_eq!(counted.frame_count, 3);
        assert_eq!(
            counted.loop_count,
            LoopCount::Finite(NonZeroU32::new(2).unwrap())
        );
    }

    #[test]
    fn simple_file_is_not_animated() {
        assert_eq!(read_header(&fixture("red-3x2.webp")[..]).unwrap(), None);
    }

    #[test]
    fn extended_file_without_animation_flag_is_not_animated() {
        let mut data = fixture("anim-rgb.webp");
        // The flags follow "RIFF", the size, "WEBP" and the VP8X header.
        data[20] &= !ANIMATION_FLAG;
        assert_eq!(read_header(&data[..]).unwrap(), None);
    }

    #[test]
    fn frame_cut_off_by_the_end_is_not_counted() {
        let data = fixture("anim-rgb.webp");
        let counted = read_header(&data[..data.len() - 1]).unwrap().unwrap();
        assert_eq!(counted.frame_count, 2);
    }

    #[test]
    fn rejects_other_formats() {
        assert!(read_header(&fixture("red-3x2.png")[..]).is_err());
    }
}
