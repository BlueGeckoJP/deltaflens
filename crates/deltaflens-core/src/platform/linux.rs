//! Linux, following the freedesktop standards.

mod instance;
mod thumbnail_cache;

use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::Command,
};

pub use self::{
    instance::{InstanceListener, hand_over, instance_socket},
    thumbnail_cache::ThumbnailCache,
};

/// The command that opens a file with the application chosen for its type.
/// `<path>` is replaced with the path of the file.
pub const DEFAULT_OPEN_COMMAND: &str = "xdg-open <path>";

/// Returns whether `path` is hidden: its name starts with a dot.
#[must_use]
pub fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.'))
}

/// The directory for files that can be made again, such as thumbnails:
/// `$XDG_CACHE_HOME`, or `~/.cache`.
#[must_use]
pub fn cache_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CACHE_HOME", ".cache")
}

/// The directory for settings: `$XDG_CONFIG_HOME`, or `~/.config`.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    xdg_dir("XDG_CONFIG_HOME", ".config")
}

/// The directory for state that is kept between runs, such as the last
/// session: `$XDG_STATE_HOME`, or `~/.local/state`.
#[must_use]
pub fn state_dir() -> Option<PathBuf> {
    xdg_dir("XDG_STATE_HOME", ".local/state")
}

/// The directory of the XDG Base Directory Specification in the environment
/// variable `var`, or else `default` in the home directory.
fn xdg_dir(var: &str, default: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .map(PathBuf::from)
        // The specification ignores relative paths.
        .filter(|dir| dir.is_absolute())
        .or_else(|| std::env::home_dir().map(|home| home.join(default)))
}

/// The command that puts PNG data from its standard input on the clipboard:
/// wl-copy on Wayland, or else xclip on X11. `None` if there is neither.
///
/// `var` reads an environment variable, like [`std::env::var_os`].
pub fn clipboard_command(var: impl Fn(&str) -> Option<OsString>) -> Option<Command> {
    let is_set = |name| var(name).is_some_and(|value| !value.is_empty());
    if is_set("WAYLAND_DISPLAY") {
        let mut command = Command::new("wl-copy");
        command.args(["--type", "image/png"]);
        Some(command)
    } else if is_set("DISPLAY") {
        let mut command = Command::new("xclip");
        command.args(["-selection", "clipboard", "-target", "image/png", "-in"]);
        Some(command)
    } else {
        None
    }
}

/// Makes the memory of large allocations, such as decoded images, go back to
/// the system when they are freed.
///
/// By default glibc raises the threshold for giving large allocations their
/// own memory mapping up to 32 MB as they are freed, and keeps the freed
/// memory of smaller ones for later. The buffers made while decoding images
/// are just below that, so the memory used grew to about twice the limit of
/// the image cache. Other C libraries are left as they are.
pub fn return_large_allocations_to_the_system() {
    #[cfg(target_env = "gnu")]
    {
        /// Allocations at least this large get their own memory mapping.
        const MMAP_THRESHOLD: libc::c_int = 1024 * 1024;
        // SAFETY: `mallopt` only changes a setting of the allocator, which
        // takes effect for later allocations.
        if unsafe { libc::mallopt(libc::M_MMAP_THRESHOLD, MMAP_THRESHOLD) } == 0 {
            tracing::warn!("failed to set the mmap threshold of the allocator");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let vars: Vec<(String, OsString)> = vars
            .iter()
            .map(|(name, value)| ((*name).to_owned(), OsString::from(value)))
            .collect();
        move |name| {
            vars.iter()
                .find(|(var, _)| var == name)
                .map(|(_, value)| value.clone())
        }
    }

    fn program_and_args(command: &Command) -> Vec<String> {
        std::iter::once(command.get_program())
            .chain(command.get_args())
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn wayland_uses_wl_copy() {
        let command =
            clipboard_command(env(&[("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")])).unwrap();
        assert_eq!(
            program_and_args(&command),
            ["wl-copy", "--type", "image/png"]
        );
    }

    #[test]
    fn x11_uses_xclip() {
        let command =
            clipboard_command(env(&[("WAYLAND_DISPLAY", ""), ("DISPLAY", ":0")])).unwrap();
        assert_eq!(
            program_and_args(&command),
            [
                "xclip",
                "-selection",
                "clipboard",
                "-target",
                "image/png",
                "-in"
            ]
        );
    }

    #[test]
    fn no_display_has_no_command() {
        assert!(clipboard_command(env(&[])).is_none());
    }
}
