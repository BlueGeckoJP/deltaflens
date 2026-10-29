//! Keeping to one running instance: a second start hands the paths it was to
//! open to the first through a Unix domain socket, and exits.

use std::{
    ffi::OsStr,
    io::{ErrorKind, Read, Write},
    net::Shutdown,
    os::unix::{
        ffi::OsStrExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use eyre::{Result, WrapErr};

/// The socket of the running instance: `deltaflens.sock` in
/// `$XDG_RUNTIME_DIR`, which only the user can reach and the system clears
/// on logout. `None` without it, so that every start is its own instance.
#[must_use]
pub fn instance_socket() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join("deltaflens.sock"))
}

/// Hands `paths` to the instance listening at `socket`. Returns whether there
/// is one; if not, this start is to become it with [`InstanceListener::bind`].
///
/// # Errors
///
/// Returns an error if the instance cannot be told.
pub fn hand_over(socket: &Path, paths: &[PathBuf]) -> Result<bool> {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        // Nobody listens: there is no socket, or one left by an instance that
        // did not exit cleanly.
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(false);
        }
        Err(error) => {
            return Err(error)
                .wrap_err_with(|| format!("failed to connect to {}", socket.display()));
        }
    };
    stream.write_all(&encode(paths))?;
    stream.shutdown(Shutdown::Write)?;
    Ok(true)
}

/// Listens for later starts that hand over their paths.
#[derive(Debug)]
pub struct InstanceListener {
    listener: UnixListener,
}

impl InstanceListener {
    /// Listens at `socket`, replacing a socket left by an instance that did
    /// not exit cleanly.
    ///
    /// # Errors
    ///
    /// Returns an error if the socket cannot be made.
    pub fn bind(socket: &Path) -> Result<Self> {
        match std::fs::remove_file(socket) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .wrap_err_with(|| format!("failed to remove {}", socket.display()));
            }
        }
        let listener = UnixListener::bind(socket)
            .wrap_err_with(|| format!("failed to listen at {}", socket.display()))?;
        Ok(Self { listener })
    }

    /// Waits for a later start and returns the paths it hands over, which may
    /// be none.
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub fn accept(&self) -> Result<Vec<PathBuf>> {
        let (mut stream, _) = self.listener.accept()?;
        let mut message = Vec::new();
        stream.read_to_end(&mut message)?;
        Ok(decode(&message))
    }
}

/// The paths, each ended with a NUL byte, which no path contains.
fn encode(paths: &[PathBuf]) -> Vec<u8> {
    let mut message = Vec::new();
    for path in paths {
        message.extend_from_slice(path.as_os_str().as_bytes());
        message.push(0);
    }
    message
}

fn decode(message: &[u8]) -> Vec<PathBuf> {
    message
        .split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| PathBuf::from(OsStr::from_bytes(path)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    #[test]
    fn message_holds_any_path() {
        let paths = vec![
            PathBuf::from("/a b/c.jxl"),
            PathBuf::from(OsStr::from_bytes(b"/\xff\n.png")),
            PathBuf::from("relative/d.png"),
        ];
        assert_eq!(decode(&encode(&paths)), paths);
    }

    #[test]
    fn empty_message_has_no_paths() {
        assert_eq!(decode(&encode(&[])), Vec::<PathBuf>::new());
    }

    #[test]
    fn nobody_listening_means_no_instance() {
        let dir = TempDir::with_files("instance-none", &[]);
        let socket = dir.path().join("deltaflens.sock");
        assert!(!hand_over(&socket, &[PathBuf::from("/a.png")]).unwrap());
    }

    #[test]
    fn paths_reach_the_running_instance() {
        let dir = TempDir::with_files("instance-running", &[]);
        let socket = dir.path().join("deltaflens.sock");
        let listener = InstanceListener::bind(&socket).unwrap();
        let paths = vec![PathBuf::from("/a.png"), PathBuf::from("/b c.jxl")];
        let sent = std::thread::spawn({
            let (socket, paths) = (socket.clone(), paths.clone());
            move || hand_over(&socket, &paths).unwrap()
        });
        assert_eq!(listener.accept().unwrap(), paths);
        assert!(sent.join().unwrap());
    }

    #[test]
    fn socket_left_by_a_killed_instance_is_replaced() {
        let dir = TempDir::with_files("instance-stale", &[]);
        let socket = dir.path().join("deltaflens.sock");
        // Dropping the listener leaves the socket file, as a killed process
        // does.
        drop(InstanceListener::bind(&socket).unwrap());
        assert!(socket.exists());
        assert!(!hand_over(&socket, &[]).unwrap());
        let listener = InstanceListener::bind(&socket).unwrap();
        let sent = std::thread::spawn({
            let socket = socket.clone();
            move || hand_over(&socket, &[]).unwrap()
        });
        assert_eq!(listener.accept().unwrap(), Vec::<PathBuf>::new());
        assert!(sent.join().unwrap());
    }
}
