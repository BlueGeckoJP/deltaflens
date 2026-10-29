//! Opening files with an external command.

use std::{ffi::OsString, path::Path, process::Command};

use eyre::{Result, bail};

/// Replaced with the path of the file in a command template.
pub const PATH_PLACEHOLDER: &str = "<path>";

/// Builds the command that `template` describes for `path`.
///
/// The template is split at whitespace, without a shell. Each
/// [`PATH_PLACEHOLDER`] is replaced with `path`; if there is none, `path` is
/// appended as the last argument. For example, `xdg-open <path>` opens the file
/// with its default application.
///
/// # Errors
///
/// Returns an error if the template is empty or starts with the placeholder.
pub fn external_command(template: &str, path: &Path) -> Result<Command> {
    let mut words = template.split_whitespace();
    let Some(program) = words.next() else {
        bail!("the command is empty");
    };
    if program.contains(PATH_PLACEHOLDER) {
        bail!("the command must start with a program, not {PATH_PLACEHOLDER}");
    }
    let mut command = Command::new(program);
    let mut has_placeholder = false;
    for word in words {
        if word.contains(PATH_PLACEHOLDER) {
            has_placeholder = true;
            command.arg(replace_placeholder(word, path));
        } else {
            command.arg(word);
        }
    }
    if !has_placeholder {
        command.arg(path);
    }
    Ok(command)
}

/// Replaces each placeholder in `word` with `path`, which need not be UTF-8.
fn replace_placeholder(word: &str, path: &Path) -> OsString {
    let mut parts = word.split(PATH_PLACEHOLDER);
    let mut arg = OsString::from(parts.next().unwrap_or_default());
    for part in parts {
        arg.push(path);
        arg.push(part);
    }
    arg
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    fn parts(command: &Command) -> (String, Vec<String>) {
        (
            command.get_program().to_string_lossy().into_owned(),
            command
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect(),
        )
    }

    #[test]
    fn placeholder_is_replaced_with_path() {
        let command = external_command("xdg-open <path>", Path::new("/a b/c.jxl")).unwrap();
        assert_eq!(
            parts(&command),
            ("xdg-open".into(), vec!["/a b/c.jxl".into()])
        );
    }

    #[test]
    fn path_is_appended_without_placeholder() {
        let command = external_command("gimp  --new-instance", Path::new("/c.png")).unwrap();
        assert_eq!(
            parts(&command),
            (
                "gimp".into(),
                vec!["--new-instance".into(), "/c.png".into()]
            )
        );
    }

    #[test]
    fn placeholder_inside_word_is_replaced() {
        let command = external_command("app --file=<path> -x", Path::new("/c.png")).unwrap();
        assert_eq!(
            parts(&command),
            ("app".into(), vec!["--file=/c.png".into(), "-x".into()])
        );
    }

    // Only Unix paths can be any bytes.
    #[cfg(unix)]
    #[test]
    fn non_utf8_path_is_kept() {
        use std::os::unix::ffi::OsStrExt;

        let path = Path::new(OsStr::from_bytes(b"/\xff.png"));
        let command = external_command("open <path>", path).unwrap();
        assert_eq!(command.get_args().collect::<Vec<_>>(), [path.as_os_str()]);
    }

    #[test]
    fn empty_command_fails() {
        assert!(external_command("  ", Path::new("/c.png")).is_err());
    }

    #[test]
    fn placeholder_as_program_fails() {
        assert!(external_command("<path> --flag", Path::new("/c.png")).is_err());
    }
}
