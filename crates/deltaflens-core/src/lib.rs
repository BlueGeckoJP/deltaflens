//! UI-independent core of deltaflens.
//!
//! This crate must not depend on gpui.

mod clipboard;
mod command;
mod decode;
mod explorer;
mod folder;
mod history;
mod id;
mod layout;
mod lru;
mod pane;
mod tab;
#[cfg(test)]
mod test_util;
mod view;
mod workspace;

pub use clipboard::{clipboard_command, png_for_clipboard, run_with_input};
pub use command::{PATH_PLACEHOLDER, external_command};
pub use decode::{DecodedImage, ThumbnailCache, decode, is_supported_image, thumbnail};
pub use explorer::{Entry, EntryKind, Sort, SortKey, SortOrder, list_entries, sort_entries};
pub use folder::{Direction, list_images, neighbor_image};
pub use history::History;
pub use id::{PaneId, TabId};
pub use layout::{Axis, Layout};
pub use lru::LruCache;
pub use pane::Pane;
pub use tab::{Tab, TabContent};
pub use view::{Point, Rect, Size, ViewTransform};
pub use workspace::Workspace;
