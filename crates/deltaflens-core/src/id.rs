/// Identifies a pane. Allocated by [`crate::Workspace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PaneId(pub(crate) u64);

/// Identifies a tab. Allocated by [`crate::Workspace`], unique across all panes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(pub(crate) u64);
