use std::path::{Path, PathBuf};

use crate::TabId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabContent {
    Viewer {
        path: PathBuf,
    },
    /// Shows the contents of `dir`.
    Explorer {
        dir: PathBuf,
    },
    /// Shows the settings and changes them.
    Settings,
}

impl TabContent {
    /// The folder that holds the image or folder shown, if any.
    #[must_use]
    pub fn folder(&self) -> Option<&Path> {
        match self {
            Self::Viewer { path } => path.parent(),
            Self::Explorer { dir } => dir.parent(),
            Self::Settings => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tab {
    id: TabId,
    content: TabContent,
}

impl Tab {
    pub(crate) fn new(id: TabId, content: TabContent) -> Self {
        Self { id, content }
    }

    #[must_use]
    pub fn id(&self) -> TabId {
        self.id
    }

    #[must_use]
    pub fn content(&self) -> &TabContent {
        &self.content
    }

    pub(crate) fn set_content(&mut self, content: TabContent) {
        self.content = content;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_holds_what_the_tab_shows() {
        let viewer = TabContent::Viewer {
            path: PathBuf::from("/photos/kyoto/torii.jxl"),
        };
        let explorer = TabContent::Explorer {
            dir: PathBuf::from("/photos/kyoto"),
        };
        assert_eq!(viewer.folder(), Some(Path::new("/photos/kyoto")));
        assert_eq!(explorer.folder(), Some(Path::new("/photos")));
        assert_eq!(TabContent::Settings.folder(), None);
    }
}
