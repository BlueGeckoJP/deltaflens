//! The contents of a folder, as the explorer lists them.

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
    time::SystemTime,
};

use eyre::{Result, WrapErr};

use crate::{folder::compare_names, is_supported_image};

/// Folders are sorted before images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    Folder,
    Image,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    pub kind: EntryKind,
    /// `None` if the file system does not record it.
    pub modified: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Natural, case-insensitive order of file names.
    #[default]
    Name,
    /// Last modification time. Entries without one come first.
    Modified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortOrder {
    #[default]
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Sort {
    pub key: SortKey,
    pub order: SortOrder,
}

/// Lists the folders and supported images in `dir`, in no particular order.
/// Hidden entries, whose names start with a dot, are left out.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
pub fn list_entries(dir: &Path) -> Result<Vec<Entry>> {
    list_entries_inner(dir).wrap_err_with(|| format!("failed to list {}", dir.display()))
}

fn list_entries_inner(dir: &Path) -> Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for entry in dir.read_dir()? {
        let path = entry?.path();
        if is_hidden(&path) {
            continue;
        }
        // `metadata` follows symbolic links. Entries it fails on, such as broken
        // links, cannot be opened anyway.
        let Ok(metadata) = path.metadata() else {
            continue;
        };
        let kind = if metadata.is_dir() {
            EntryKind::Folder
        } else if metadata.is_file() && is_supported_image(&path) {
            EntryKind::Image
        } else {
            continue;
        };
        entries.push(Entry {
            path,
            kind,
            modified: metadata.modified().ok(),
        });
    }
    Ok(entries)
}

/// Returns whether the name of `path` starts with a dot.
pub(crate) fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name.starts_with('.'))
}

/// Sorts `entries` by `sort`, with folders first in either order.
pub fn sort_entries(entries: &mut [Entry], sort: Sort) {
    entries.sort_by(|a, b| {
        let order = match sort.key {
            SortKey::Name => compare_names(&a.path, &b.path),
            SortKey::Modified => a
                .modified
                .cmp(&b.modified)
                .then_with(|| compare_names(&a.path, &b.path)),
        };
        let order = match sort.order {
            SortOrder::Ascending => order,
            SortOrder::Descending => order.reverse(),
        };
        a.kind.cmp(&b.kind).then(order)
    });
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use super::*;
    use crate::test_util::TempDir;

    fn entry(name: &str, kind: EntryKind, modified: Option<u64>) -> Entry {
        Entry {
            path: PathBuf::from("/d").join(name),
            kind,
            modified: modified.map(|secs| SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
        }
    }

    fn names(entries: &[Entry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| {
                entry
                    .path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }

    fn sorted(mut entries: Vec<Entry>, key: SortKey, order: SortOrder) -> Vec<String> {
        sort_entries(&mut entries, Sort { key, order });
        names(&entries)
    }

    #[test]
    fn list_entries_returns_folders_and_images() {
        let dir = TempDir::with_files("explorer-list", &["a.png", "b.JXL", "notes.txt", "c"]);
        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::create_dir(dir.path().join("folder.png")).unwrap();
        let mut entries = list_entries(dir.path()).unwrap();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        let paths: Vec<_> = entries.iter().map(|entry| entry.path.clone()).collect();
        let kinds: Vec<_> = entries.iter().map(|entry| entry.kind).collect();
        assert_eq!(dir.names(&paths), ["a.png", "b.JXL", "folder.png", "sub"]);
        assert_eq!(
            kinds,
            [
                EntryKind::Image,
                EntryKind::Image,
                EntryKind::Folder,
                EntryKind::Folder
            ]
        );
        assert!(entries.iter().all(|entry| entry.modified.is_some()));
    }

    #[test]
    fn list_entries_skips_hidden_entries() {
        let dir = TempDir::with_files("explorer-hidden", &[".hidden.png", "shown.png"]);
        fs::create_dir(dir.path().join(".git")).unwrap();
        let entries = list_entries(dir.path()).unwrap();
        assert_eq!(names(&entries), ["shown.png"]);
    }

    #[test]
    fn list_entries_of_missing_directory_fails() {
        let dir = TempDir::with_files("explorer-missing", &[]);
        assert!(list_entries(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn sort_by_name_puts_folders_first() {
        let entries = vec![
            entry("b.png", EntryKind::Image, None),
            entry("img10", EntryKind::Folder, None),
            entry("A.png", EntryKind::Image, None),
            entry("img2", EntryKind::Folder, None),
        ];
        assert_eq!(
            sorted(entries.clone(), SortKey::Name, SortOrder::Ascending),
            ["img2", "img10", "A.png", "b.png"]
        );
        assert_eq!(
            sorted(entries, SortKey::Name, SortOrder::Descending),
            ["img10", "img2", "b.png", "A.png"]
        );
    }

    #[test]
    fn sort_by_modified_puts_folders_first() {
        let entries = vec![
            entry("new.png", EntryKind::Image, Some(30)),
            entry("old", EntryKind::Folder, Some(10)),
            entry("old.png", EntryKind::Image, Some(10)),
            entry("new", EntryKind::Folder, Some(20)),
        ];
        assert_eq!(
            sorted(entries.clone(), SortKey::Modified, SortOrder::Ascending),
            ["old", "new", "old.png", "new.png"]
        );
        assert_eq!(
            sorted(entries, SortKey::Modified, SortOrder::Descending),
            ["new", "old", "new.png", "old.png"]
        );
    }

    #[test]
    fn sort_by_modified_breaks_ties_by_name() {
        let entries = vec![
            entry("b.png", EntryKind::Image, Some(10)),
            entry("a.png", EntryKind::Image, Some(10)),
            entry("unknown.png", EntryKind::Image, None),
        ];
        assert_eq!(
            sorted(entries, SortKey::Modified, SortOrder::Ascending),
            ["unknown.png", "a.png", "b.png"]
        );
    }
}
