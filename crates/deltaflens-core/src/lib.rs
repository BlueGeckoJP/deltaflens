//! UI-independent core of deltaflens.
//!
//! This crate must not depend on gpui.

mod decode;
mod id;
mod layout;
mod pane;
mod tab;
mod workspace;

pub use decode::{DecodedImage, decode, is_supported_image};
pub use id::{PaneId, TabId};
pub use layout::{Axis, Layout};
pub use pane::Pane;
pub use tab::{Tab, TabContent};
pub use workspace::Workspace;
