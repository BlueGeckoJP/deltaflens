//! Keeping to one running instance: a second start hands the paths it was to
//! open to the first through a Unix domain socket, and exits.

use std::{
    ffi::OsStr,
    fs::{File, TryLockError},
    io::{ErrorKind, Read, Write},
    net::Shutdown,
    os::unix::{
        ffi::OsStrExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use eyre::{Result, WrapErr, bail};

/// How many times a start tries to hand over its paths while another one that
/// holds the lock is starting to listen.
const HAND_OVER_ATTEMPTS: u32 = 50;
/// How long it waits between the attempts.
const HAND_OVER_INTERVAL: Duration = Duration::from_millis(100);

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

/// What a start is to do.
#[derive(Debug)]
pub enum Claim {
    /// Its paths were handed to the running instance, so it is to exit.
    HandedOver,
    /// It is the running instance now.
    Listening(InstanceListener),
}

/// Hands `paths` to the instance listening at `socket`, or else becomes that
/// instance.
///
/// Only the start that holds the lock beside the socket listens, so that two
/// starts at the same time do not both become the instance. A start that
/// cannot take the lock waits for the one that did to listen.
///
/// # Errors
///
/// Returns an error if the instance cannot be told, or the socket cannot be
/// made.
pub fn claim(socket: &Path, paths: &[PathBuf]) -> Result<Claim> {
    let lock_path = socket.with_extension("lock");
    for _ in 0..HAND_OVER_ATTEMPTS {
        if hand_over(socket, paths)? {
            return Ok(Claim::HandedOver);
        }
        let lock = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .wrap_err_with(|| format!("failed to open {}", lock_path.display()))?;
        match lock.try_lock() {
            Ok(()) => return InstanceListener::bind(socket, lock).map(Claim::Listening),
            Err(TryLockError::WouldBlock) => thread::sleep(HAND_OVER_INTERVAL),
            Err(TryLockError::Error(error)) => {
                return Err(error)
                    .wrap_err_with(|| format!("failed to lock {}", lock_path.display()));
            }
        }
    }
    bail!(
        "another instance holds {} but does not listen",
        lock_path.display()
    )
}

/// Hands `paths` to the instance listening at `socket`. Returns whether there
/// is one.
fn hand_over(socket: &Path, paths: &[PathBuf]) -> Result<bool> {
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
    /// Held while listening. The system lets go of it when the process ends,
    /// however it ends.
    _lock: File,
}

impl InstanceListener {
    /// Listens at `socket`, replacing a socket left by an instance that did
    /// not exit cleanly. `lock` must be locked, so that no other instance
    /// listens there.
    fn bind(socket: &Path, lock: File) -> Result<Self> {
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
        Ok(Self {
            listener,
            _lock: lock,
        })
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

    fn socket(dir: &TempDir) -> PathBuf {
        dir.path().join("deltaflens.sock")
    }

    fn listening(claim: Claim) -> InstanceListener {
        match claim {
            Claim::Listening(listener) => listener,
            Claim::HandedOver => panic!("handed over"),
        }
    }

    #[test]
    fn first_start_listens() {
        let dir = TempDir::with_files("instance-first", &[]);
        assert!(!hand_over(&socket(&dir), &[PathBuf::from("/a.png")]).unwrap());
        listening(claim(&socket(&dir), &[]).unwrap());
    }

    #[test]
    fn paths_reach_the_running_instance() {
        let dir = TempDir::with_files("instance-running", &[]);
        let listener = listening(claim(&socket(&dir), &[]).unwrap());
        let paths = vec![PathBuf::from("/a.png"), PathBuf::from("/b c.jxl")];
        let claimed = thread::spawn({
            let (socket, paths) = (socket(&dir), paths.clone());
            move || claim(&socket, &paths).unwrap()
        });
        assert_eq!(listener.accept().unwrap(), paths);
        assert!(matches!(claimed.join().unwrap(), Claim::HandedOver));
    }

    #[test]
    fn start_waits_for_the_one_holding_the_lock() {
        let dir = TempDir::with_files("instance-starting", &[]);
        // Another start holds the lock but listens only later.
        let lock = File::create(socket(&dir).with_extension("lock")).unwrap();
        lock.try_lock().unwrap();
        let claimed = thread::spawn({
            let socket = socket(&dir);
            move || claim(&socket, &[PathBuf::from("/a.png")]).unwrap()
        });
        thread::sleep(HAND_OVER_INTERVAL * 3);
        let listener = InstanceListener::bind(&socket(&dir), lock).unwrap();
        assert!(matches!(claimed.join().unwrap(), Claim::HandedOver));
        assert_eq!(listener.accept().unwrap(), [PathBuf::from("/a.png")]);
    }

    #[test]
    fn socket_left_by_a_killed_instance_is_replaced() {
        let dir = TempDir::with_files("instance-stale", &[]);
        // Dropping the listener leaves the socket file and lets go of the
        // lock, as a killed process does.
        drop(listening(claim(&socket(&dir), &[]).unwrap()));
        assert!(socket(&dir).exists());
        let listener = listening(claim(&socket(&dir), &[]).unwrap());
        let claimed = thread::spawn({
            let socket = socket(&dir);
            move || claim(&socket, &[]).unwrap()
        });
        assert_eq!(listener.accept().unwrap(), Vec::<PathBuf>::new());
        assert!(matches!(claimed.join().unwrap(), Claim::HandedOver));
    }
}
