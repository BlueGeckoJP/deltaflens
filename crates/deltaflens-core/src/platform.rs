//! What is done differently on each operating system. Only Linux is
//! supported; another system gets a module of its own beside `linux`, chosen
//! with `cfg(target_os)`, with the same items.

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "linux")]
pub use self::linux::*;
