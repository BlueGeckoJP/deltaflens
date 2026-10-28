use std::{
    cmp::Ordering,
    path::{Path, PathBuf},
};

use eyre::{Result, WrapErr};

use crate::is_supported_image;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Previous,
    Next,
}

/// Lists the files in `dir` that [`is_supported_image`] accepts, in natural
/// order of their file names.
///
/// # Errors
///
/// Returns an error if the directory cannot be read.
pub fn list_images(dir: &Path) -> Result<Vec<PathBuf>> {
    list_images_inner(dir).wrap_err_with(|| format!("failed to list {}", dir.display()))
}

fn list_images_inner(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut images = Vec::new();
    for entry in dir.read_dir()? {
        let path = entry?.path();
        // `is_file` follows symbolic links.
        if is_supported_image(&path) && path.is_file() {
            images.push(path);
        }
    }
    images.sort_by(|a, b| compare_names(a, b));
    Ok(images)
}

/// Returns the image next to `current` in `images`, which must be sorted by
/// [`list_images`]. Wraps around at both ends.
///
/// If `current` is not in `images`, returns the image that would be next to
/// it. Returns `None` if `images` is empty.
#[must_use]
pub fn neighbor_image<'a>(
    images: &'a [PathBuf],
    current: &Path,
    direction: Direction,
) -> Option<&'a Path> {
    if images.is_empty() {
        return None;
    }
    let index = images.binary_search_by(|image| compare_names(image, current));
    let next = match (index, direction) {
        (Ok(index), Direction::Next) => index + 1,
        // The image at the insertion point is the next one.
        (Err(index), Direction::Next) => index,
        (Ok(index) | Err(index), Direction::Previous) => {
            index.checked_sub(1).unwrap_or(images.len() - 1)
        }
    };
    Some(&images[next % images.len()])
}

/// Natural, case-insensitive order of file names. Ties are broken so that the
/// order is total.
pub(crate) fn compare_names(a: &Path, b: &Path) -> Ordering {
    let (a, b) = (
        a.file_name().unwrap_or_default(),
        b.file_name().unwrap_or_default(),
    );
    let (a_lossy, b_lossy) = (a.to_string_lossy(), b.to_string_lossy());
    natord::compare_ignore_case(&a_lossy, &b_lossy)
        .then_with(|| natord::compare(&a_lossy, &b_lossy))
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::test_util::TempDir;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from("/d").join(name))
            .collect()
    }

    fn neighbor(images: &[PathBuf], current: &str, direction: Direction) -> Option<String> {
        neighbor_image(images, &Path::new("/d").join(current), direction)
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
    }

    #[test]
    fn list_images_in_natural_order() {
        let dir = TempDir::with_files("folder-natural", &["img10.png", "img2.png", "img1.png"]);
        let images = list_images(dir.path()).unwrap();
        assert_eq!(dir.names(&images), ["img1.png", "img2.png", "img10.png"]);
    }

    #[test]
    fn list_images_ignores_case() {
        let dir = TempDir::with_files("folder-case", &["b.png", "A.png", "c.JPG"]);
        let images = list_images(dir.path()).unwrap();
        assert_eq!(dir.names(&images), ["A.png", "b.png", "c.JPG"]);
    }

    #[test]
    fn list_images_skips_unsupported_files_and_directories() {
        let dir = TempDir::with_files("folder-unsupported", &["a.png", "notes.txt", "b.avif", "c"]);
        fs::create_dir(dir.path().join("folder.png")).unwrap();
        let images = list_images(dir.path()).unwrap();
        assert_eq!(dir.names(&images), ["a.png"]);
    }

    #[test]
    fn list_images_of_empty_directory() {
        let dir = TempDir::with_files("folder-empty", &[]);
        assert_eq!(list_images(dir.path()).unwrap(), Vec::<PathBuf>::new());
    }

    #[test]
    fn list_images_of_missing_directory_fails() {
        let dir = TempDir::with_files("folder-missing", &[]);
        assert!(list_images(&dir.path().join("missing")).is_err());
    }

    #[test]
    fn neighbor_moves_by_one() {
        let images = paths(&["a.png", "b.png", "c.png"]);
        assert_eq!(
            neighbor(&images, "b.png", Direction::Next).unwrap(),
            "c.png"
        );
        assert_eq!(
            neighbor(&images, "b.png", Direction::Previous).unwrap(),
            "a.png"
        );
    }

    #[test]
    fn neighbor_wraps_around() {
        let images = paths(&["a.png", "b.png", "c.png"]);
        assert_eq!(
            neighbor(&images, "c.png", Direction::Next).unwrap(),
            "a.png"
        );
        assert_eq!(
            neighbor(&images, "a.png", Direction::Previous).unwrap(),
            "c.png"
        );
    }

    #[test]
    fn neighbor_follows_natural_order() {
        let images = paths(&["img2.png", "img10.png"]);
        assert_eq!(
            neighbor(&images, "img2.png", Direction::Next).unwrap(),
            "img10.png"
        );
    }

    #[test]
    fn neighbor_of_missing_image_uses_its_position() {
        let images = paths(&["a.png", "c.png", "e.png"]);
        assert_eq!(
            neighbor(&images, "d.png", Direction::Next).unwrap(),
            "e.png"
        );
        assert_eq!(
            neighbor(&images, "d.png", Direction::Previous).unwrap(),
            "c.png"
        );
        assert_eq!(
            neighbor(&images, "f.png", Direction::Next).unwrap(),
            "a.png"
        );
        assert_eq!(
            neighbor(&images, "0.png", Direction::Previous).unwrap(),
            "e.png"
        );
    }

    #[test]
    fn neighbor_of_only_image_is_itself() {
        let images = paths(&["a.png"]);
        assert_eq!(
            neighbor(&images, "a.png", Direction::Next).unwrap(),
            "a.png"
        );
        assert_eq!(
            neighbor(&images, "a.png", Direction::Previous).unwrap(),
            "a.png"
        );
    }

    #[test]
    fn neighbor_in_empty_list_is_none() {
        assert_eq!(neighbor(&[], "a.png", Direction::Next), None);
    }
}
