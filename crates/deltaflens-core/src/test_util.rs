//! Helpers shared by the unit tests.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// A directory under the system temporary directory, removed on drop.
pub(crate) struct TempDir(PathBuf);

impl TempDir {
    /// Creates the directory with empty `files`. `name` must be unique among
    /// the tests.
    pub(crate) fn with_files(name: &str, files: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!("deltaflens-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        for file in files {
            fs::write(dir.join(file), b"").unwrap();
        }
        Self(dir)
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The file names of `paths`, which must be in this directory.
    pub(crate) fn names(&self, paths: &[PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| {
                assert_eq!(path.parent(), Some(self.0.as_path()));
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
