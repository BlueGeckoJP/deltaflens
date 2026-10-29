use std::path::PathBuf;

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
