use eyre::{Result, eyre};

use crate::{Tab, TabId};

/// A row of tabs with at most one active tab.
#[derive(Debug, Clone, Default)]
pub struct Pane {
    tabs: Vec<Tab>,
    /// Index into `tabs`. `None` if and only if `tabs` is empty.
    active: Option<usize>,
}

impl Pane {
    #[must_use]
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }

    #[must_use]
    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.map(|index| &self.tabs[index])
    }

    /// Inserts `tab` to the right of the active tab and activates it.
    pub(crate) fn add_tab(&mut self, tab: Tab) {
        let index = self.active.map_or(0, |active| active + 1);
        self.tabs.insert(index, tab);
        self.active = Some(index);
    }

    /// Closes the tab. If it was active, the tab to its right (or left, if there
    /// is none) becomes active.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane has no tab with `id`.
    pub fn close_tab(&mut self, id: TabId) -> Result<()> {
        let index = self.index_of(id)?;
        self.tabs.remove(index);
        self.active = match self.active {
            _ if self.tabs.is_empty() => None,
            Some(active) if index < active => Some(active - 1),
            // The right neighbor has moved into `index`.
            Some(active) if index == active => Some(index.min(self.tabs.len() - 1)),
            active => active,
        };
        Ok(())
    }

    /// # Errors
    ///
    /// Returns an error if the pane has no tab with `id`.
    pub fn activate_tab(&mut self, id: TabId) -> Result<()> {
        self.active = Some(self.index_of(id)?);
        Ok(())
    }

    /// Activates the next tab, wrapping around at the end. Does nothing if the
    /// pane has no tabs.
    pub fn activate_next_tab(&mut self) {
        if let Some(active) = self.active {
            self.active = Some((active + 1) % self.tabs.len());
        }
    }

    /// Activates the previous tab, wrapping around at the start. Does nothing if
    /// the pane has no tabs.
    pub fn activate_previous_tab(&mut self) {
        if let Some(active) = self.active {
            self.active = Some(active.checked_sub(1).unwrap_or(self.tabs.len() - 1));
        }
    }

    fn index_of(&self, id: TabId) -> Result<usize> {
        self.tabs
            .iter()
            .position(|tab| tab.id() == id)
            .ok_or_else(|| eyre!("tab {id:?} is not in the pane"))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::TabContent;

    fn tab(id: u64) -> Tab {
        Tab::new(
            TabId(id),
            TabContent::Viewer {
                path: PathBuf::from(format!("{id}.jxl")),
            },
        )
    }

    /// Creates a pane with tabs `0..count`, the last one active.
    fn pane_with_tabs(count: u64) -> Pane {
        let mut pane = Pane::default();
        for id in 0..count {
            pane.add_tab(tab(id));
        }
        pane
    }

    fn tab_ids(pane: &Pane) -> Vec<u64> {
        pane.tabs().iter().map(|tab| tab.id().0).collect()
    }

    fn active_id(pane: &Pane) -> Option<u64> {
        pane.active_tab().map(|tab| tab.id().0)
    }

    #[test]
    fn new_pane_is_empty() {
        let pane = Pane::default();
        assert_eq!(pane.tabs(), []);
        assert_eq!(active_id(&pane), None);
    }

    #[test]
    fn add_tab_inserts_right_of_active_and_activates() {
        let mut pane = pane_with_tabs(3);
        pane.activate_tab(TabId(0)).unwrap();
        pane.add_tab(tab(3));
        assert_eq!(tab_ids(&pane), vec![0, 3, 1, 2]);
        assert_eq!(active_id(&pane), Some(3));
    }

    #[test]
    fn close_active_tab_activates_right_neighbor() {
        let mut pane = pane_with_tabs(3);
        pane.activate_tab(TabId(1)).unwrap();
        pane.close_tab(TabId(1)).unwrap();
        assert_eq!(tab_ids(&pane), vec![0, 2]);
        assert_eq!(active_id(&pane), Some(2));
    }

    #[test]
    fn close_rightmost_active_tab_activates_left_neighbor() {
        let mut pane = pane_with_tabs(3);
        pane.close_tab(TabId(2)).unwrap();
        assert_eq!(tab_ids(&pane), vec![0, 1]);
        assert_eq!(active_id(&pane), Some(1));
    }

    #[test]
    fn close_tab_left_of_active_keeps_active() {
        let mut pane = pane_with_tabs(3);
        pane.close_tab(TabId(0)).unwrap();
        assert_eq!(tab_ids(&pane), vec![1, 2]);
        assert_eq!(active_id(&pane), Some(2));
    }

    #[test]
    fn close_tab_right_of_active_keeps_active() {
        let mut pane = pane_with_tabs(3);
        pane.activate_tab(TabId(0)).unwrap();
        pane.close_tab(TabId(2)).unwrap();
        assert_eq!(tab_ids(&pane), vec![0, 1]);
        assert_eq!(active_id(&pane), Some(0));
    }

    #[test]
    fn close_last_tab_leaves_pane_empty() {
        let mut pane = pane_with_tabs(1);
        pane.close_tab(TabId(0)).unwrap();
        assert_eq!(pane.tabs(), []);
        assert_eq!(active_id(&pane), None);
    }

    #[test]
    fn close_tab_in_empty_pane_fails() {
        let mut pane = Pane::default();
        assert!(pane.close_tab(TabId(0)).is_err());
    }

    #[test]
    fn close_missing_tab_fails_without_change() {
        let mut pane = pane_with_tabs(2);
        assert!(pane.close_tab(TabId(9)).is_err());
        assert_eq!(tab_ids(&pane), vec![0, 1]);
        assert_eq!(active_id(&pane), Some(1));
    }

    #[test]
    fn activate_missing_tab_fails_without_change() {
        let mut pane = pane_with_tabs(2);
        assert!(pane.activate_tab(TabId(9)).is_err());
        assert_eq!(active_id(&pane), Some(1));
    }

    #[test]
    fn activate_next_tab_wraps_around() {
        let mut pane = pane_with_tabs(3);
        pane.activate_tab(TabId(1)).unwrap();
        pane.activate_next_tab();
        assert_eq!(active_id(&pane), Some(2));
        pane.activate_next_tab();
        assert_eq!(active_id(&pane), Some(0));
    }

    #[test]
    fn activate_previous_tab_wraps_around() {
        let mut pane = pane_with_tabs(3);
        pane.activate_tab(TabId(1)).unwrap();
        pane.activate_previous_tab();
        assert_eq!(active_id(&pane), Some(0));
        pane.activate_previous_tab();
        assert_eq!(active_id(&pane), Some(2));
    }

    #[test]
    fn activate_next_and_previous_in_empty_pane_do_nothing() {
        let mut pane = Pane::default();
        pane.activate_next_tab();
        pane.activate_previous_tab();
        assert_eq!(active_id(&pane), None);
    }
}
