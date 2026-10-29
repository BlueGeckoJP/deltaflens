//! UI-independent core of deltaflens.
//!
//! This crate must not depend on gpui.

mod clipboard;
mod command;
mod decode;
mod downscale;
mod explorer;
mod folder;
mod history;
mod id;
mod layout;
mod lru;
mod pane;
pub mod platform;
mod preset;
mod tab;
#[cfg(test)]
mod test_util;
mod tiles;
mod view;
mod workspace;

pub use clipboard::{png_for_clipboard, run_with_input};
pub use command::{PATH_PLACEHOLDER, external_command};
pub use decode::{DecodedImage, decode, is_supported_image, thumbnail};
pub use downscale::{downscale, downscaled_size};
pub use explorer::{Entry, EntryKind, Sort, SortKey, SortOrder, list_entries, sort_entries};
pub use folder::{Direction, list_images, neighbor_image, prefetch_images};
pub use history::History;
pub use id::{PaneId, TabId};
pub use layout::{Axis, DropZone, Layout, Side, drop_zone};
pub use lru::LruCache;
pub use pane::Pane;
pub use preset::Preset;
pub use tab::{Tab, TabContent};
pub use tiles::{TileRect, cut_tiles, overview_size, tile_rects, visible_tiles};
pub use view::{Point, Rect, Size, ViewTransform};
pub use workspace::Workspace;
