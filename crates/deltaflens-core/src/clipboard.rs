//! Copying images to the clipboard with a command of the system, as gpui
//! writes only text to the clipboard on Linux. The command is
//! [`crate::platform::clipboard_command`].

use std::{
    fs::{self, File},
    io::{ErrorKind, Read, Write},
    path::Path,
    process::{Command, Stdio},
};

use eyre::{Result, WrapErr, bail, eyre};

use crate::decode::{decode, encode_png};

const PNG_SIGNATURE: [u8; 8] = *b"\x89PNG\r\n\x1a\n";

/// The image at `path` as PNG data: the file itself if it is a PNG, or else
/// the decoded image encoded as PNG.
///
/// # Errors
///
/// Returns an error if the file cannot be read or decoded.
pub fn png_for_clipboard(path: &Path) -> Result<Vec<u8>> {
    let mut signature = [0; PNG_SIGNATURE.len()];
    let is_png = match File::open(path)?.read_exact(&mut signature) {
        Ok(()) => signature == PNG_SIGNATURE,
        Err(error) if error.kind() == ErrorKind::UnexpectedEof => false,
        Err(error) => return Err(error.into()),
    };
    if is_png {
        return Ok(fs::read(path)?);
    }
    let image = decode(path)?;
    let mut png = Vec::new();
    // Speed matters more than size, as the data only goes to another process.
    encode_png(&mut png, &image, &[], png::Compression::Fast)?;
    Ok(png)
}

/// Runs `command` with `input` as its standard input and waits for it.
///
/// Clipboard commands such as wl-copy and xclip leave a process behind that
/// serves the clipboard, so its output is not captured: reading it would wait
/// for that process too.
///
/// # Errors
///
/// Returns an error if the command is not installed, cannot be run, or fails.
pub fn run_with_input(mut command: Command, input: &[u8]) -> Result<()> {
    let program = command.get_program().to_string_lossy().into_owned();
    let mut child = match command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == ErrorKind::NotFound => {
            bail!("{program} is not installed")
        }
        Err(error) => return Err(error).wrap_err_with(|| format!("failed to run {program}")),
    };
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| eyre!("no standard input for {program}"))?;
    let written = stdin.write_all(input);
    // Closing it tells the command that the input is complete.
    drop(stdin);
    let status = child.wait()?;
    written.wrap_err_with(|| format!("failed to write to {program}"))?;
    if !status.success() {
        bail!("{program} failed ({status})");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::test_util::TempDir;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    #[test]
    fn png_file_is_copied_as_is() {
        let path = fixture("red-3x2.png");
        assert_eq!(png_for_clipboard(&path).unwrap(), fs::read(&path).unwrap());
    }

    #[test]
    fn other_formats_are_encoded_as_png() {
        let png = png_for_clipboard(&fixture("red-3x2.jxl")).unwrap();
        assert!(png.starts_with(&PNG_SIGNATURE));
        let image = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(image.dimensions(), (3, 2));
        assert!(image.pixels().all(|pixel| pixel.0 == [255, 0, 0, 255]));
    }

    #[test]
    fn short_file_is_not_taken_for_png() {
        let dir = TempDir::with_files("clipboard-short", &["empty.png"]);
        assert!(png_for_clipboard(&dir.path().join("empty.png")).is_err());
    }

    #[test]
    fn run_with_input_passes_the_input() {
        let dir = TempDir::with_files("clipboard-run", &[]);
        let out = dir.path().join("out");
        let mut command = Command::new("sh");
        command.args(["-c", "cat > \"$0\""]).arg(&out);
        run_with_input(command, b"data").unwrap();
        assert_eq!(fs::read(&out).unwrap(), b"data");
    }

    #[test]
    fn run_with_input_reports_failure() {
        let error = run_with_input(Command::new("false"), b"").unwrap_err();
        assert!(error.to_string().contains("false failed"), "{error:#}");
    }

    #[test]
    fn run_with_input_reports_missing_program() {
        let error = run_with_input(Command::new("deltaflens-no-such-program"), b"").unwrap_err();
        assert_eq!(
            error.to_string(),
            "deltaflens-no-such-program is not installed"
        );
    }
}
