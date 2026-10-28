use std::path::{Path, PathBuf};

/// The folders visited in an explorer, for going back and forward like a web
/// browser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    /// The oldest folder first.
    back: Vec<PathBuf>,
    current: PathBuf,
    /// The folder to go forward to last.
    forward: Vec<PathBuf>,
}

impl History {
    #[must_use]
    pub fn new(dir: PathBuf) -> Self {
        Self {
            back: Vec::new(),
            current: dir,
            forward: Vec::new(),
        }
    }

    #[must_use]
    pub fn current(&self) -> &Path {
        &self.current
    }

    #[must_use]
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    #[must_use]
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Moves to `dir` and forgets the folders to go forward to. Does nothing if
    /// `dir` is the current folder.
    pub fn navigate(&mut self, dir: PathBuf) {
        if dir == self.current {
            return;
        }
        self.back.push(std::mem::replace(&mut self.current, dir));
        self.forward.clear();
    }

    /// Returns whether it moved.
    pub fn go_back(&mut self) -> bool {
        let Some(dir) = self.back.pop() else {
            return false;
        };
        self.forward.push(std::mem::replace(&mut self.current, dir));
        true
    }

    /// Returns whether it moved.
    pub fn go_forward(&mut self) -> bool {
        let Some(dir) = self.forward.pop() else {
            return false;
        };
        self.back.push(std::mem::replace(&mut self.current, dir));
        true
    }

    /// Moves to the parent folder, as [`History::navigate`] does. Returns
    /// whether it moved, which it does not at the root.
    pub fn go_up(&mut self) -> bool {
        match self.current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => {
                self.navigate(parent.to_owned());
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(dirs: &[&str]) -> History {
        let mut history = History::new(PathBuf::from(dirs[0]));
        for dir in &dirs[1..] {
            history.navigate(PathBuf::from(dir));
        }
        history
    }

    fn current(history: &History) -> &str {
        history.current().to_str().unwrap()
    }

    #[test]
    fn new_history_cannot_move() {
        let mut history = history(&["/a"]);
        assert!(!history.can_go_back());
        assert!(!history.can_go_forward());
        assert!(!history.go_back());
        assert!(!history.go_forward());
        assert_eq!(current(&history), "/a");
    }

    #[test]
    fn back_and_forward_retrace_navigation() {
        let mut history = history(&["/a", "/b", "/c"]);
        assert!(history.go_back());
        assert_eq!(current(&history), "/b");
        assert!(history.go_back());
        assert_eq!(current(&history), "/a");
        assert!(!history.go_back());
        assert!(history.go_forward());
        assert_eq!(current(&history), "/b");
        assert!(history.go_forward());
        assert_eq!(current(&history), "/c");
        assert!(!history.go_forward());
    }

    #[test]
    fn navigate_clears_forward() {
        let mut history = history(&["/a", "/b"]);
        history.go_back();
        history.navigate(PathBuf::from("/c"));
        assert!(!history.can_go_forward());
        assert!(history.go_back());
        assert_eq!(current(&history), "/a");
    }

    #[test]
    fn navigate_to_current_folder_does_nothing() {
        let mut history = history(&["/a", "/b"]);
        history.go_back();
        history.navigate(PathBuf::from("/a"));
        assert!(history.can_go_forward());
        assert!(!history.can_go_back());
    }

    #[test]
    fn up_moves_to_parent_and_can_go_back() {
        let mut history = history(&["/a/b"]);
        assert!(history.go_up());
        assert_eq!(current(&history), "/a");
        assert!(history.go_back());
        assert_eq!(current(&history), "/a/b");
    }

    #[test]
    fn up_at_root_does_nothing() {
        let mut root = history(&["/"]);
        assert!(!root.go_up());
        assert!(!root.can_go_back());
        let mut relative = history(&["relative"]);
        assert!(!relative.go_up());
    }
}
