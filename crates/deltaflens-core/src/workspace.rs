use std::collections::{HashMap, VecDeque};

use eyre::{Result, eyre};

use crate::{Axis, Layout, Pane, PaneId, Preset, Side, Tab, TabContent, TabId};

/// How many closed tabs are remembered to reopen.
const MAX_CLOSED_TABS: usize = 20;

/// A closed tab, remembered to reopen it where it was.
#[derive(Debug, Clone, PartialEq)]
struct ClosedTab {
    content: TabContent,
    pane_id: PaneId,
    index: usize,
}

/// The layout of a window, its panes and the focused pane.
#[derive(Debug, Clone)]
pub struct Workspace {
    layout: Layout,
    panes: HashMap<PaneId, Pane>,
    focused_pane: PaneId,
    /// Shared by pane and tab IDs.
    next_id: u64,
    /// The most recently closed tab last.
    closed_tabs: VecDeque<ClosedTab>,
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
            closed_tabs: VecDeque::new(),
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

    /// Closes the tab and remembers it for [`Self::reopen_closed_tab`]. A pane
    /// left without tabs is closed too, unless it is the only pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane has no tab with `tab_id`.
    pub fn close_tab(&mut self, pane_id: PaneId, tab_id: TabId) -> Result<()> {
        let pane = self.pane_mut(pane_id)?;
        let index = pane.tabs().iter().position(|tab| tab.id() == tab_id);
        let tab = pane.take_tab(tab_id)?;
        if self.closed_tabs.len() == MAX_CLOSED_TABS {
            self.closed_tabs.pop_front();
        }
        self.closed_tabs.push_back(ClosedTab {
            content: tab.content().clone(),
            pane_id,
            index: index.unwrap_or_default(),
        });
        self.close_pane_if_empty(pane_id)
    }

    /// Reopens the most recently closed tab whose content still `exists`, and
    /// forgets it and the ones skipped. It goes back to where it was if its
    /// pane is still open, and next to the active tab of the focused pane
    /// otherwise. Its pane is focused.
    ///
    /// Returns the reopened tab, or `None` if there was none to reopen.
    pub fn reopen_closed_tab(&mut self, exists: impl Fn(&TabContent) -> bool) -> Option<TabId> {
        while let Some(closed) = self.closed_tabs.pop_back() {
            if !exists(&closed.content) {
                continue;
            }
            let tab_id = TabId(self.allocate_id());
            let tab = Tab::new(tab_id, closed.content);
            if let Some(pane) = self.panes.get_mut(&closed.pane_id) {
                pane.insert_tab(closed.index, tab);
                self.focused_pane = closed.pane_id;
            } else {
                self.panes.get_mut(&self.focused_pane)?.add_tab(tab);
            }
            return Some(tab_id);
        }
        None
    }

    /// Whether [`Self::reopen_closed_tab`] would reopen a tab.
    #[must_use]
    pub fn can_reopen_closed_tab(&self, exists: impl Fn(&TabContent) -> bool) -> bool {
        self.closed_tabs
            .iter()
            .any(|closed| exists(&closed.content))
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

    /// Changes the layout to `preset`, keeping every tab.
    ///
    /// The panes go into the new layout in order. If there are too few, the
    /// new panes get a copy of the active tab of the focused pane. If there are
    /// too many, the tabs of the extra panes move to the last pane.
    ///
    /// For [`Preset::ExplorerAndViewer`], when the focused tab is a viewer, it
    /// moves to the right and the left shows its folder in an explorer. When
    /// it is an explorer, it moves to the left. New panes start empty instead
    /// of with a copy, so that the viewer is not shown twice.
    pub fn apply_preset(&mut self, preset: Preset) {
        let focused_tab = self
            .pane(self.focused_pane)
            .ok()
            .and_then(Pane::active_tab)
            .map(|tab| (tab.id(), tab.content().clone()));
        let count = preset.pane_count();
        let mut ids = self.layout.pane_ids();
        let extra = ids.split_off(count.min(ids.len()));
        if let Some(&last) = ids.last() {
            for id in extra {
                let tabs = self
                    .panes
                    .remove(&id)
                    .map_or_default(|mut pane| pane.take_tabs());
                if let Some(pane) = self.panes.get_mut(&last) {
                    pane.append_tabs(tabs);
                }
                if self.focused_pane == id {
                    self.focused_pane = last;
                }
            }
        }
        let copy = focused_tab
            .as_ref()
            .filter(|_| preset != Preset::ExplorerAndViewer)
            .map(|(_, content)| content.clone());
        while ids.len() < count {
            let id = PaneId(self.allocate_id());
            let mut pane = Pane::default();
            if let Some(content) = &copy {
                pane.add_tab(Tab::new(TabId(self.allocate_id()), content.clone()));
            }
            self.panes.insert(id, pane);
            ids.push(id);
        }
        self.layout = preset.layout(&ids);
        if preset == Preset::ExplorerAndViewer
            && let Some((tab_id, content)) = focused_tab
        {
            self.arrange_explorer_and_viewer(ids[0], ids[1], tab_id, content);
        }
    }

    /// Puts the focused tab where [`Preset::ExplorerAndViewer`] wants it.
    fn arrange_explorer_and_viewer(
        &mut self,
        left: PaneId,
        right: PaneId,
        tab_id: TabId,
        content: TabContent,
    ) {
        match content {
            TabContent::Viewer { path } => {
                self.move_tab_to(tab_id, right);
                self.focused_pane = right;
                let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) else {
                    return;
                };
                let explorer = TabContent::Explorer {
                    dir: dir.to_path_buf(),
                };
                let existing = self.panes.get(&left).and_then(|pane| {
                    pane.tabs()
                        .iter()
                        .find(|tab| *tab.content() == explorer)
                        .map(Tab::id)
                });
                match existing {
                    Some(id) => {
                        if let Some(pane) = self.panes.get_mut(&left) {
                            let _ = pane.activate_tab(id);
                        }
                    }
                    None => {
                        let _ = self.open_tab(left, explorer);
                    }
                }
            }
            TabContent::Explorer { .. } => {
                self.move_tab_to(tab_id, left);
                self.focused_pane = left;
            }
            // Shown in the larger pane, like an image.
            TabContent::Settings => {
                self.move_tab_to(tab_id, right);
                self.focused_pane = right;
            }
        }
    }

    /// Moves the tab to `to`, to the right of its active tab, and activates it
    /// there. Does nothing if the tab or the pane does not exist.
    fn move_tab_to(&mut self, tab_id: TabId, to: PaneId) {
        let _ = self.transfer_tab(tab_id, to, None);
    }

    /// Moves the tab to `index` in `to` (or to the right of its active tab),
    /// and activates it there. A pane left empty stays.
    fn transfer_tab(&mut self, tab_id: TabId, to: PaneId, index: Option<usize>) -> Result<PaneId> {
        let from = self
            .pane_of_tab(tab_id)
            .ok_or_else(|| eyre!("tab {tab_id:?} is not open"))?;
        self.pane(to)?;
        if from == to {
            let pane = self.pane_mut(to)?;
            if let Some(index) = index {
                pane.move_tab(tab_id, index)?;
            }
            pane.activate_tab(tab_id)?;
            return Ok(from);
        }
        let tab = self.pane_mut(from)?.take_tab(tab_id)?;
        let pane = self.pane_mut(to)?;
        match index {
            Some(index) => pane.insert_tab(index, tab),
            None => pane.add_tab(tab),
        }
        Ok(from)
    }

    /// Closes `pane_id` if it has no tabs, unless it is the only pane.
    fn close_pane_if_empty(&mut self, pane_id: PaneId) -> Result<()> {
        if self.pane(pane_id)?.tabs().is_empty() && self.layout != Layout::Pane(pane_id) {
            self.close_pane(pane_id)?;
        }
        Ok(())
    }

    /// Moves the tab to `index` in `to` (or to the right of its active tab),
    /// activates it and focuses `to`. A pane left without tabs is closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the tab is not open or `to` does not exist.
    pub fn move_tab(&mut self, tab_id: TabId, to: PaneId, index: Option<usize>) -> Result<()> {
        let from = self.transfer_tab(tab_id, to, index)?;
        self.focused_pane = to;
        self.close_pane_if_empty(from)
    }

    /// Splits `target` toward `side` and moves the tab into the new pane, which
    /// is focused. A pane left without tabs is closed.
    ///
    /// # Errors
    ///
    /// Returns an error if the tab is not open or `target` does not exist.
    pub fn move_tab_to_split(
        &mut self,
        tab_id: TabId,
        target: PaneId,
        side: Side,
    ) -> Result<PaneId> {
        self.pane_of_tab(tab_id)
            .ok_or_else(|| eyre!("tab {tab_id:?} is not open"))?;
        let new_id = self.split_empty(target, side)?;
        self.move_tab(tab_id, new_id, None)?;
        Ok(new_id)
    }

    /// Splits `target` toward `side` and opens `content` in the new pane, which
    /// is focused.
    ///
    /// # Errors
    ///
    /// Returns an error if `target` does not exist.
    pub fn open_tab_in_split(
        &mut self,
        target: PaneId,
        side: Side,
        content: TabContent,
    ) -> Result<TabId> {
        let new_id = self.split_empty(target, side)?;
        self.focused_pane = new_id;
        self.open_tab(new_id, content)
    }

    /// Splits `target` toward `side` with a new, empty pane.
    fn split_empty(&mut self, target: PaneId, side: Side) -> Result<PaneId> {
        self.pane(target)?;
        let new_id = PaneId(self.allocate_id());
        self.layout.split_toward(target, side, new_id)?;
        self.panes.insert(new_id, Pane::default());
        Ok(new_id)
    }

    /// Swaps the tabs of two panes. The panes stay where they are in the
    /// layout, with their sizes.
    ///
    /// # Errors
    ///
    /// Returns an error if either pane does not exist.
    pub fn swap_panes(&mut self, a: PaneId, b: PaneId) -> Result<()> {
        if a == b {
            return self.pane(a).map(|_| ());
        }
        let pane_a = self.panes.remove(&a).ok_or_else(|| missing_pane(a))?;
        let Some(pane_b) = self.panes.remove(&b) else {
            self.panes.insert(a, pane_a);
            return Err(missing_pane(b));
        };
        self.panes.insert(a, pane_b);
        self.panes.insert(b, pane_a);
        Ok(())
    }

    /// Swaps the tabs of the focused pane with those of its neighbor on `side`,
    /// and focuses the neighbor, which now has the tabs that were focused.
    /// Returns whether there was a neighbor.
    pub fn swap_with_neighbor(&mut self, side: Side) -> bool {
        let focused = self.focused_pane;
        let Some(neighbor) = self.layout.neighbor(focused, side) else {
            return false;
        };
        if self.swap_panes(focused, neighbor).is_err() {
            return false;
        }
        self.focused_pane = neighbor;
        true
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

    /// Activates the tab and focuses its pane.
    ///
    /// # Errors
    ///
    /// Returns an error if the tab is not open.
    pub fn activate_tab(&mut self, tab_id: TabId) -> Result<()> {
        let pane_id = self
            .pane_of_tab(tab_id)
            .ok_or_else(|| eyre!("tab {tab_id:?} is not open"))?;
        self.pane_mut(pane_id)?.activate_tab(tab_id)?;
        self.focused_pane = pane_id;
        Ok(())
    }

    /// The first open tab that shows `content`, if any.
    #[must_use]
    pub fn find_tab(&self, content: &TabContent) -> Option<TabId> {
        self.layout.pane_ids().into_iter().find_map(|pane_id| {
            self.panes
                .get(&pane_id)?
                .tabs()
                .iter()
                .find(|tab| tab.content() == content)
                .map(Tab::id)
        })
    }

    /// The pane that has the tab, if any.
    #[must_use]
    pub fn pane_of_tab(&self, tab_id: TabId) -> Option<PaneId> {
        self.panes
            .iter()
            .find(|(_, pane)| pane.tabs().iter().any(|tab| tab.id() == tab_id))
            .map(|(&pane_id, _)| pane_id)
    }

    /// Replaces the layout, the panes and the focused pane, for restoring a
    /// session. `panes` must hold the panes of `layout`.
    pub(crate) fn replace(&mut self, layout: Layout, panes: Vec<(PaneId, Pane)>, focused: PaneId) {
        self.layout = layout;
        self.panes = panes.into_iter().collect();
        self.focused_pane = focused;
        self.closed_tabs.clear();
    }

    /// Closes the panes without tabs, unless every pane is empty, in which case
    /// one is kept.
    pub(crate) fn close_empty_panes(&mut self) {
        let empty: Vec<PaneId> = self
            .layout
            .pane_ids()
            .into_iter()
            .filter(|id| self.panes.get(id).is_none_or(|pane| pane.tabs().is_empty()))
            .collect();
        for id in empty {
            if self.layout.pane_ids().len() == 1 {
                break;
            }
            if let Ok(neighbor) = self.layout.remove(id) {
                self.panes.remove(&id);
                if self.focused_pane == id {
                    self.focused_pane = neighbor;
                }
            }
        }
    }

    pub(crate) fn allocate_id(&mut self) -> u64 {
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

    fn explorer(dir: &str) -> TabContent {
        TabContent::Explorer {
            dir: PathBuf::from(dir),
        }
    }

    fn active_content(workspace: &Workspace, pane_id: PaneId) -> Option<TabContent> {
        workspace
            .pane(pane_id)
            .unwrap()
            .active_tab()
            .map(|tab| tab.content().clone())
    }

    /// Every tab of the workspace, pane by pane.
    fn all_contents(workspace: &Workspace) -> Vec<Vec<TabContent>> {
        workspace
            .layout()
            .pane_ids()
            .into_iter()
            .map(|id| contents(workspace, id))
            .collect()
    }

    #[test]
    fn preset_with_more_panes_copies_the_focused_tab() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        workspace.open_tab(first, viewer("b.jxl")).unwrap();

        workspace.apply_preset(Preset::ThreeColumns);

        assert_eq!(
            all_contents(&workspace),
            [
                vec![viewer("a.jxl"), viewer("b.jxl")],
                vec![viewer("b.jxl")],
                vec![viewer("b.jxl")],
            ]
        );
        assert_eq!(workspace.layout().pane_ids()[0], first);
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn preset_with_fewer_panes_moves_the_extra_tabs_to_the_last_pane() {
        let mut workspace = Workspace::new();
        workspace.apply_preset(Preset::ThreeColumns);
        let ids = workspace.layout().pane_ids();
        for (id, name) in ids.iter().zip(["a.jxl", "b.jxl", "c.jxl"]) {
            workspace.open_tab(*id, viewer(name)).unwrap();
        }
        workspace.focus_pane(ids[2]).unwrap();

        workspace.apply_preset(Preset::Single);

        assert_eq!(workspace.layout(), &Layout::Pane(ids[0]));
        assert_eq!(
            contents(&workspace, ids[0]),
            [viewer("a.jxl"), viewer("b.jxl"), viewer("c.jxl")]
        );
        assert_eq!(active_content(&workspace, ids[0]), Some(viewer("a.jxl")));
        assert_eq!(workspace.focused_pane(), ids[0]);
        assert!(workspace.pane(ids[2]).is_err());
    }

    #[test]
    fn preset_with_the_same_number_of_panes_keeps_the_tabs() {
        let mut workspace = Workspace::new();
        workspace.apply_preset(Preset::TwoColumns);
        let ids = workspace.layout().pane_ids();
        workspace.open_tab(ids[0], viewer("a.jxl")).unwrap();
        workspace.open_tab(ids[1], viewer("b.jxl")).unwrap();

        workspace.apply_preset(Preset::TwoColumns);

        assert_eq!(workspace.layout().pane_ids(), ids);
        assert_eq!(
            all_contents(&workspace),
            [vec![viewer("a.jxl")], vec![viewer("b.jxl")]]
        );
    }

    #[test]
    fn preset_of_an_empty_workspace_has_empty_panes() {
        let mut workspace = Workspace::new();
        workspace.apply_preset(Preset::Grid);
        assert_eq!(all_contents(&workspace), vec![Vec::new(); 4]);
    }

    #[test]
    fn explorer_and_viewer_from_a_viewer() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, explorer("/pictures")).unwrap();
        workspace
            .open_tab(first, viewer("/pictures/a.jxl"))
            .unwrap();

        workspace.apply_preset(Preset::ExplorerAndViewer);

        let ids = workspace.layout().pane_ids();
        // The explorer that already shows the folder is reused.
        assert_eq!(contents(&workspace, ids[0]), [explorer("/pictures")]);
        assert_eq!(
            active_content(&workspace, ids[0]),
            Some(explorer("/pictures"))
        );
        assert_eq!(contents(&workspace, ids[1]), [viewer("/pictures/a.jxl")]);
        assert_eq!(workspace.focused_pane(), ids[1]);
    }

    #[test]
    fn explorer_and_viewer_from_a_single_viewer_opens_an_explorer() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace
            .open_tab(first, viewer("/pictures/a.jxl"))
            .unwrap();

        workspace.apply_preset(Preset::ExplorerAndViewer);

        assert_eq!(
            all_contents(&workspace),
            [vec![explorer("/pictures")], vec![viewer("/pictures/a.jxl")]]
        );
    }

    #[test]
    fn explorer_and_viewer_puts_settings_on_the_right() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, TabContent::Settings).unwrap();

        workspace.apply_preset(Preset::ExplorerAndViewer);

        assert_eq!(
            all_contents(&workspace),
            [vec![], vec![TabContent::Settings]]
        );
    }

    #[test]
    fn find_and_activate_a_tab_in_another_pane() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let settings = workspace.open_tab(first, TabContent::Settings).unwrap();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        assert_eq!(workspace.focused_pane(), second);

        assert_eq!(workspace.find_tab(&TabContent::Settings), Some(settings));
        assert_eq!(workspace.find_tab(&viewer("b.jxl")), None);
        workspace.activate_tab(settings).unwrap();
        assert_eq!(workspace.focused_pane(), first);
        assert_eq!(
            active_content(&workspace, first),
            Some(TabContent::Settings)
        );
        assert!(workspace.activate_tab(TabId(999)).is_err());
    }

    #[test]
    fn explorer_and_viewer_opens_the_folder_of_the_image() {
        let mut workspace = Workspace::new();
        workspace.apply_preset(Preset::TwoColumns);
        let ids = workspace.layout().pane_ids();
        workspace
            .open_tab(ids[0], viewer("/pictures/a.jxl"))
            .unwrap();
        workspace.open_tab(ids[1], viewer("/other/b.jxl")).unwrap();
        workspace.focus_pane(ids[0]).unwrap();

        workspace.apply_preset(Preset::ExplorerAndViewer);

        assert_eq!(contents(&workspace, ids[0]), [explorer("/pictures")]);
        assert_eq!(
            contents(&workspace, ids[1]),
            [viewer("/other/b.jxl"), viewer("/pictures/a.jxl")]
        );
        assert_eq!(
            active_content(&workspace, ids[1]),
            Some(viewer("/pictures/a.jxl"))
        );
    }

    #[test]
    fn explorer_and_viewer_from_an_explorer() {
        let mut workspace = Workspace::new();
        workspace.apply_preset(Preset::TwoColumns);
        let ids = workspace.layout().pane_ids();
        workspace
            .open_tab(ids[0], viewer("/pictures/a.jxl"))
            .unwrap();
        workspace.open_tab(ids[1], explorer("/pictures")).unwrap();
        workspace
            .open_tab(ids[1], viewer("/pictures/b.jxl"))
            .unwrap();
        let pane = workspace.pane_mut(ids[1]).unwrap();
        let explorer_tab = pane.tabs()[0].id();
        pane.activate_tab(explorer_tab).unwrap();
        workspace.focus_pane(ids[1]).unwrap();

        workspace.apply_preset(Preset::ExplorerAndViewer);

        assert_eq!(
            contents(&workspace, ids[0]),
            [viewer("/pictures/a.jxl"), explorer("/pictures")]
        );
        assert_eq!(
            active_content(&workspace, ids[0]),
            Some(explorer("/pictures"))
        );
        assert_eq!(contents(&workspace, ids[1]), [viewer("/pictures/b.jxl")]);
        assert_eq!(workspace.focused_pane(), ids[0]);
    }

    /// Two panes side by side: `a.jxl` on the left, `a.jxl` and `b.jxl` on the
    /// right. Returns the panes and the tab of `b.jxl`.
    fn two_panes() -> (Workspace, PaneId, PaneId, TabId) {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        let b = workspace.open_tab(second, viewer("b.jxl")).unwrap();
        (workspace, first, second, b)
    }

    #[test]
    fn move_tab_to_another_pane() {
        let (mut workspace, first, second, b) = two_panes();
        workspace.move_tab(b, first, Some(0)).unwrap();
        assert_eq!(
            contents(&workspace, first),
            [viewer("b.jxl"), viewer("a.jxl")]
        );
        assert_eq!(active_content(&workspace, first), Some(viewer("b.jxl")));
        assert_eq!(contents(&workspace, second), [viewer("a.jxl")]);
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn move_last_tab_closes_its_pane() {
        let (mut workspace, first, second, b) = two_panes();
        let a = workspace.pane(second).unwrap().tabs()[0].id();
        workspace.move_tab(a, first, None).unwrap();
        workspace.move_tab(b, first, None).unwrap();
        assert_eq!(workspace.layout(), &Layout::Pane(first));
        assert_eq!(contents(&workspace, first).len(), 3);
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn move_tab_within_its_pane_reorders() {
        let (mut workspace, _, second, b) = two_panes();
        workspace.move_tab(b, second, Some(0)).unwrap();
        assert_eq!(
            contents(&workspace, second),
            [viewer("b.jxl"), viewer("a.jxl")]
        );
    }

    #[test]
    fn move_tab_to_a_split() {
        let (mut workspace, first, second, b) = two_panes();
        let new = workspace.move_tab_to_split(b, first, Side::Down).unwrap();
        assert_eq!(workspace.layout().neighbor(first, Side::Down), Some(new));
        assert_eq!(contents(&workspace, new), [viewer("b.jxl")]);
        assert_eq!(contents(&workspace, second), [viewer("a.jxl")]);
        assert_eq!(workspace.focused_pane(), new);
    }

    #[test]
    fn move_only_tab_to_a_split_of_its_own_pane() {
        // Splitting off the only tab of a pane leaves the old pane empty, so it
        // closes and the tab ends up in a pane of its own at the same place.
        let (mut workspace, first, second, b) = two_panes();
        let a = workspace.pane(second).unwrap().tabs()[0].id();
        workspace.close_tab(second, a).unwrap();
        let new = workspace.move_tab_to_split(b, second, Side::Left).unwrap();
        assert_eq!(workspace.layout().pane_ids(), vec![first, new]);
        assert_eq!(contents(&workspace, new), [viewer("b.jxl")]);
    }

    #[test]
    fn open_tab_in_a_split() {
        let (mut workspace, first, _, _) = two_panes();
        let tab = workspace
            .open_tab_in_split(first, Side::Left, viewer("c.jxl"))
            .unwrap();
        let new = workspace.pane_of_tab(tab).unwrap();
        assert_eq!(workspace.layout().neighbor(first, Side::Left), Some(new));
        assert_eq!(workspace.focused_pane(), new);
    }

    #[test]
    fn move_missing_tab_fails() {
        let (mut workspace, first, _, _) = two_panes();
        assert!(workspace.move_tab(TabId(99), first, None).is_err());
        assert!(
            workspace
                .move_tab_to_split(TabId(99), first, Side::Left)
                .is_err()
        );
        assert_eq!(workspace.layout().pane_ids().len(), 2);
    }

    fn always(_: &TabContent) -> bool {
        true
    }

    #[test]
    fn reopen_closed_tabs_newest_first_at_their_place() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let a = workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let b = workspace.open_tab(first, viewer("b.jxl")).unwrap();
        workspace.open_tab(first, viewer("c.jxl")).unwrap();
        workspace.close_tab(first, a).unwrap();
        workspace.close_tab(first, b).unwrap();

        workspace.reopen_closed_tab(always).unwrap();
        assert_eq!(
            contents(&workspace, first),
            [viewer("b.jxl"), viewer("c.jxl")]
        );
        assert_eq!(active_content(&workspace, first), Some(viewer("b.jxl")));

        workspace.reopen_closed_tab(always).unwrap();
        assert_eq!(
            contents(&workspace, first),
            [viewer("a.jxl"), viewer("b.jxl"), viewer("c.jxl")]
        );
        assert_eq!(workspace.reopen_closed_tab(always), None);
    }

    #[test]
    fn closed_tabs_are_limited() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        for index in 0..MAX_CLOSED_TABS + 5 {
            let tab = workspace
                .open_tab(first, viewer(&format!("{index}.jxl")))
                .unwrap();
            workspace.close_tab(first, tab).unwrap();
        }
        let mut reopened = 0;
        while workspace.reopen_closed_tab(always).is_some() {
            reopened += 1;
        }
        assert_eq!(reopened, MAX_CLOSED_TABS);
        // The oldest were forgotten; the last one reopened is the oldest kept.
        assert_eq!(active_content(&workspace, first), Some(viewer("5.jxl")));
    }

    #[test]
    fn reopen_tab_of_a_closed_pane_in_the_focused_pane() {
        let (mut workspace, first, second, b) = two_panes();
        let a = workspace.pane(second).unwrap().tabs()[0].id();
        workspace.close_tab(second, a).unwrap();
        workspace.close_tab(second, b).unwrap();
        assert!(workspace.pane(second).is_err());

        workspace.reopen_closed_tab(always).unwrap();

        assert_eq!(
            contents(&workspace, first),
            [viewer("a.jxl"), viewer("b.jxl")]
        );
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn reopen_skips_tabs_whose_file_is_gone() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        let a = workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let b = workspace.open_tab(first, viewer("gone.jxl")).unwrap();
        workspace.close_tab(first, a).unwrap();
        workspace.close_tab(first, b).unwrap();

        let exists = |content: &TabContent| *content != viewer("gone.jxl");
        workspace.reopen_closed_tab(exists).unwrap();
        assert_eq!(contents(&workspace, first), [viewer("a.jxl")]);
        assert_eq!(workspace.reopen_closed_tab(exists), None);
    }

    #[test]
    fn can_reopen_only_tabs_whose_file_is_there() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        assert!(!workspace.can_reopen_closed_tab(always));

        let a = workspace.open_tab(first, viewer("gone.jxl")).unwrap();
        workspace.close_tab(first, a).unwrap();
        let exists = |content: &TabContent| *content != viewer("gone.jxl");
        assert!(workspace.can_reopen_closed_tab(always));
        assert!(!workspace.can_reopen_closed_tab(exists));
    }

    #[test]
    fn reopen_into_the_pane_it_was_in() {
        let (mut workspace, first, second, b) = two_panes();
        workspace.close_tab(second, b).unwrap();
        workspace.focus_pane(first).unwrap();
        workspace.reopen_closed_tab(always).unwrap();
        assert_eq!(
            contents(&workspace, second),
            [viewer("a.jxl"), viewer("b.jxl")]
        );
        assert_eq!(workspace.focused_pane(), second);
    }

    #[test]
    fn swap_with_neighbor_in_the_same_split() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        workspace.open_tab(second, viewer("b.jxl")).unwrap();
        let layout = workspace.layout().clone();

        assert!(workspace.swap_with_neighbor(Side::Left));

        assert_eq!(workspace.layout(), &layout);
        assert_eq!(
            contents(&workspace, first),
            [viewer("a.jxl"), viewer("b.jxl")]
        );
        assert_eq!(contents(&workspace, second), [viewer("a.jxl")]);
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn swap_with_neighbor_across_splits() {
        // first | (second / third), swapping third with first.
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();
        let third = workspace.split_pane(second, Axis::Vertical).unwrap();
        workspace.open_tab(third, viewer("c.jxl")).unwrap();

        assert!(workspace.swap_with_neighbor(Side::Left));

        assert_eq!(
            contents(&workspace, first),
            [viewer("a.jxl"), viewer("c.jxl")]
        );
        assert_eq!(contents(&workspace, third), [viewer("a.jxl")]);
        assert_eq!(contents(&workspace, second), [viewer("a.jxl")]);
        assert_eq!(workspace.focused_pane(), first);
    }

    #[test]
    fn swap_without_neighbor_changes_nothing() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        let second = workspace.split_pane(first, Axis::Horizontal).unwrap();

        assert!(!workspace.swap_with_neighbor(Side::Right));
        assert!(!workspace.swap_with_neighbor(Side::Up));

        assert_eq!(contents(&workspace, first), [viewer("a.jxl")]);
        assert_eq!(contents(&workspace, second), [viewer("a.jxl")]);
        assert_eq!(workspace.focused_pane(), second);
    }

    #[test]
    fn swap_missing_pane_fails_without_change() {
        let mut workspace = Workspace::new();
        let first = workspace.focused_pane();
        workspace.open_tab(first, viewer("a.jxl")).unwrap();
        assert!(workspace.swap_panes(first, PaneId(99)).is_err());
        assert!(workspace.swap_panes(PaneId(99), first).is_err());
        assert_eq!(contents(&workspace, first), [viewer("a.jxl")]);
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
