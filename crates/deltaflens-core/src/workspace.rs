use std::collections::HashMap;

use eyre::{Result, eyre};

use crate::{Axis, Layout, Pane, PaneId, Side, Tab, TabContent, TabId};

/// The layout of a window, its panes and the focused pane.
#[derive(Debug, Clone)]
pub struct Workspace {
    layout: Layout,
    panes: HashMap<PaneId, Pane>,
    focused_pane: PaneId,
    /// Shared by pane and tab IDs.
    next_id: u64,
}

impl Workspace {
    /// Creates a workspace with a single pane and no tabs.
    #[must_use]
    pub fn new() -> Self {
        let pane_id = PaneId(0);
        Self {
            layout: Layout::Pane(pane_id),
            panes: HashMap::from([(pane_id, Pane::default())]),
            focused_pane: pane_id,
            next_id: 1,
        }
    }

    #[must_use]
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    #[must_use]
    pub fn focused_pane(&self) -> PaneId {
        self.focused_pane
    }

    /// # Errors
    ///
    /// Returns an error if there is no pane with `id`.
    pub fn pane(&self, id: PaneId) -> Result<&Pane> {
        self.panes.get(&id).ok_or_else(|| missing_pane(id))
    }

    /// # Errors
    ///
    /// Returns an error if there is no pane with `id`.
    pub fn pane_mut(&mut self, id: PaneId) -> Result<&mut Pane> {
        self.panes.get_mut(&id).ok_or_else(|| missing_pane(id))
    }

    /// # Errors
    ///
    /// Returns an error if there is no pane with `id`.
    pub fn focus_pane(&mut self, id: PaneId) -> Result<()> {
        self.pane(id)?;
        self.focused_pane = id;
        Ok(())
    }

    /// Splits the pane along `axis` and focuses the new pane, which gets a
    /// copy of the active tab of the split pane, if any.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no pane with `id`.
    pub fn split_pane(&mut self, id: PaneId, axis: Axis) -> Result<PaneId> {
        let copy = self.pane(id)?.active_tab().map(|tab| tab.content().clone());
        let new_id = PaneId(self.allocate_id());
        self.layout.split(id, axis, new_id)?;
        self.panes.insert(new_id, Pane::default());
        self.focused_pane = new_id;
        if let Some(content) = copy {
            self.open_tab(new_id, content)?;
        }
        Ok(new_id)
    }

    /// Closes the pane with its tabs. If it was focused, the pane that takes
    /// its place is focused.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no pane with `id`, or if it is the only
    /// pane.
    pub fn close_pane(&mut self, id: PaneId) -> Result<()> {
        self.pane(id)?;
        let neighbor = self.layout.remove(id)?;
        self.panes.remove(&id);
        if self.focused_pane == id {
            self.focused_pane = neighbor;
        }
        Ok(())
    }

    /// Closes the tab. A pane left without tabs is closed too, unless it is the
    /// only pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane has no tab with `tab_id`.
    pub fn close_tab(&mut self, pane_id: PaneId, tab_id: TabId) -> Result<()> {
        let pane = self.pane_mut(pane_id)?;
        pane.close_tab(tab_id)?;
        if pane.tabs().is_empty() && self.layout != Layout::Pane(pane_id) {
            self.close_pane(pane_id)?;
        }
        Ok(())
    }

    /// See [`Layout::move_boundary`].
    ///
    /// # Errors
    ///
    /// Returns an error if there is no split at `path`, or no boundary after
    /// child `boundary`.
    pub fn move_boundary(
        &mut self,
        path: &[usize],
        boundary: usize,
        offset: f32,
        extent: f32,
        min_pane: f32,
    ) -> Result<()> {
        self.layout
            .move_boundary(path, boundary, offset, extent, min_pane)
    }

    /// See [`Layout::equalize`].
    ///
    /// # Errors
    ///
    /// Returns an error if there is no split at `path`.
    pub fn equalize(&mut self, path: &[usize]) -> Result<()> {
        self.layout.equalize(path)
    }

    /// Focuses the pane next to the focused pane on `side`. Returns whether
    /// there was one.
    pub fn focus_neighbor(&mut self, side: Side) -> bool {
        match self.layout.neighbor(self.focused_pane, side) {
            Some(neighbor) => {
                self.focused_pane = neighbor;
                true
            }
            None => false,
        }
    }

    /// Opens a new tab in the pane, to the right of its active tab.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no pane with `pane_id`.
    pub fn open_tab(&mut self, pane_id: PaneId, content: TabContent) -> Result<TabId> {
        self.pane(pane_id)?;
        let tab_id = TabId(self.allocate_id());
        self.pane_mut(pane_id)?.add_tab(Tab::new(tab_id, content));
        Ok(tab_id)
    }

    /// The pane that has the tab, if any.
    #[must_use]
    pub fn pane_of_tab(&self, tab_id: TabId) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_, pane)| pane.tabs().iter().any(|tab| tab.id() == tab_id))
            .map(|(&pane_id, _)| pane_id)
    }

    fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}

impl Default for Workspace {
    fn default() -> Self {
        Self::new()
    }
}

fn missing_pane(id: PaneId) -> eyre::Report {
    eyre!("pane {id:?} does not exist")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn viewer(path: &str) -> TabContent {
        TabContent::Viewer {
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn new_workspace_has_one_empty_focused_pane() {
        let workspace = Workspace::new();
        let ids = workspace.layout().pane_ids();
        assert_eq!(ids.len(), 1);
        assert_eq!(workspace.focused_pane(), ids[0]);
        assert_eq!(workspace.pane(ids[0]).unwrap().tabs(), []);
    }

    fn contents(workspace: &Workspace, pane_id: PaneId) -> Vec<TabContent> {
        workspace
            .pane(pane_id)
            .unwrap()
            .tabs()
            .iter()
            .map(|tab| tab.content().clone())
            .collect()
    }

    #[test]
    fn split_pane_copies_the_active_tab_into_the_new_focused_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let active = workspace.open_tab(first, viewer("b.jxl")).unwrap();

        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();

        assert_ne!(first, second);
        assert_eq!(workspace.layout().pane_ids(), vec![first, second]);
        assert_eq!(workspace.focused_pane(), second);
        assert_eq!(contents(&workspace, second), vec![viewer("b.jxl")]);
        assert_ne!(
            workspace.pane(second).unwrap().active_tab().unwrap().id(),
            active
        );
        assert_eq!(
            contents(&workspace, first),
            vec![viewer("a.jxl"), viewer("b.jxl")]
        );
    }

    #[test]
    fn split_empty_pane_adds_empty_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let second = workspace.split_pane(first, Axis::Vertical).unwrap();
        assert_eq!(contents(&workspace, second), []);
    }

    #[test]
    fn close_pane_focuses_the_pane_taking_its_place() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        workspace.close_pane(second).unwrap();
        assert_eq!(workspace.layout(), &Layout::Pane(first));
        assert_eq!(workspace.focused_pane(), first);
        assert!(workspace.pane(second).is_err());
    }

    #[test]
    fn close_unfocused_pane_keeps_focus() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        let third = workspace.split_pane(second, Axis::Horizontal).unwrap();
        workspace.close_pane(first).unwrap();
        assert_eq!(workspace.layout().pane_ids(), vec![second, third]);
        assert_eq!(workspace.focused_pane(), third);
    }

    #[test]
    fn close_only_pane_fails() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        assert!(workspace.close_pane(first).is_err());
        assert!(workspace.pane(first).is_ok());
    }

    #[test]
    fn closing_the_last_tab_closes_the_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        let tab = workspace.pane(second).unwrap().tabs()[0].id();
        workspace.close_tab(second, tab).unwrap();
        assert_eq!(workspace.layout(), &Layout::Pane(first));
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn closing_the_last_tab_of_the_only_pane_keeps_the_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let tab = workspace.open_tab(first, viewer("a.jxl")).unwrap();
        workspace.close_tab(first, tab).unwrap();
        assert_eq!(workspace.layout(), &Layout::Pane(first));
        assert_eq!(contents(&workspace, first), []);
    }

    #[test]
    fn closing_a_tab_that_is_not_the_last_keeps_the_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        workspace.open_tab(second, viewer("b.jxl")).unwrap();
        let tab = workspace.pane(second).unwrap().tabs()[0].id();
        workspace.close_tab(second, tab).unwrap();
        assert_eq!(workspace.layout().pane_ids(), vec![first, second]);
        assert_eq!(contents(&workspace, second), vec![viewer("b.jxl")]);
    }

    #[test]
    fn close_missing_tab_fails() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        assert!(workspace.close_tab(first, TabId(99)).is_err());
    }

    #[test]
    fn focus_neighbor_moves_focus() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        assert!(workspace.focus_neighbor(Side::Left));
        assert_eq!(workspace.focused_pane(), first);
        assert!(!workspace.focus_neighbor(Side::Left));
        assert_eq!(workspace.focused_pane(), first);
        assert!(workspace.focus_neighbor(Side::Right));
        assert_eq!(workspace.focused_pane(), second);
    }

    #[test]
    fn split_missing_pane_fails_without_change() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        assert!(workspace.split_pane(PaneId(99), Axis::Vertical).is_err());
        assert_eq!(workspace.layout(), &Layout::Pane(first));
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn open_tab_activates_new_tab() {
        let mut workspace = Workspace::new();
        let pane_id = workspace.focused_pane();
        let tab_id = workspace.open_tab(pane_id, viewer("a.jxl")).unwrap();
        let pane = workspace.pane(pane_id).unwrap();
        let active = pane.active_tab().unwrap();
        assert_eq!(active.id(), tab_id);
        assert_eq!(active.content(), &viewer("a.jxl"));
    }

    #[test]
    fn open_tab_in_missing_pane_fails() {
        let mut workspace = Workspace::new();
        assert!(workspace.open_tab(PaneId(99), viewer("a.jxl")).is_err());
    }

    #[test]
    fn ids_are_unique_across_panes_and_tabs() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let tab_a = workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Vertical).unwrap();
        let tab_b = workspace.open_tab(second, viewer("b.jxl")).unwrap();
        let tab_c = workspace.open_tab(first, viewer("c.jxl")).unwrap();

        assert_ne!(first, second);
        assert_ne!(tab_a, tab_b);
        assert_ne!(tab_b, tab_c);
        assert_ne!(tab_a, tab_c);
    }

    #[test]
    fn focus_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.split_pane(first, Axis::Horizontal).unwrap();
        workspace.focus_pane(first).unwrap();
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn focus_missing_pane_fails_without_change() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        assert!(workspace.focus_pane(PaneId(99)).is_err());
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn pane_lookup_of_missing_pane_fails() {
        let mut workspace = Workspace::new();
        assert!(workspace.pane(PaneId(99)).is_err());
        assert!(workspace.pane_mut(PaneId(99)).is_err());
    }

    #[test]
    fn pane_of_tab_finds_the_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let a = workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        let b = workspace.open_tab(second, viewer("b.jxl")).unwrap();
        assert_eq!(workspace.pane_of_tab(a), Some(first));
        assert_eq!(workspace.pane_of_tab(b), Some(second));
        assert_eq!(workspace.pane_of_tab(TabId(99)), None);
    }
}
