use eyre::{Result, eyre};

use crate::PaneId;

/// The direction in which the children of a split are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children are placed side by side, left to right.
    Horizontal,
    /// Children are stacked, top to bottom.
    Vertical,
}

/// The split tree of panes.
#[derive(Debug, Clone, PartialEq)]
pub enum Layout {
    Pane(PaneId),
    /// Each child is paired with its ratio. The ratios add up to 1.0.
    Split {
        axis: Axis,
        children: Vec<(Layout, f32)>,
    },
}

impl Layout {
    /// Splits `target` along `axis` and places `new_pane` right after it.
    ///
    /// If `target` is already in a split along the same axis, `new_pane` is added
    /// as its sibling instead of nesting a new split. The two panes share the
    /// ratio `target` had.
    ///
    /// # Errors
    ///
    /// Returns an error if `target` is not in the layout.
    pub fn split(&mut self, target: PaneId, axis: Axis, new_pane: PaneId) -> Result<()> {
        if self.split_inner(target, axis, new_pane) {
            Ok(())
        } else {
            Err(eyre!("pane {target:?} is not in the layout"))
        }
    }

    fn split_inner(&mut self, target: PaneId, axis: Axis, new_pane: PaneId) -> bool {
        match self {
            Layout::Pane(id) => {
                if *id != target {
                    return false;
                }
                *self = Layout::Split {
                    axis,
                    children: vec![(Layout::Pane(target), 0.5), (Layout::Pane(new_pane), 0.5)],
                };
                true
            }
            Layout::Split {
                axis: split_axis,
                children,
            } => {
                if *split_axis == axis
                    && let Some(index) = children
                        .iter()
                        .position(|(child, _)| *child == Layout::Pane(target))
                {
                    let half = children[index].1 / 2.0;
                    children[index].1 = half;
                    children.insert(index + 1, (Layout::Pane(new_pane), half));
                    return true;
                }
                children
                    .iter_mut()
                    .any(|(child, _)| child.split_inner(target, axis, new_pane))
            }
        }
    }

    /// Returns the IDs of all panes, depth first, from left (top) to right (bottom).
    #[must_use]
    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.collect_pane_ids(&mut ids);
        ids
    }

    fn collect_pane_ids(&self, ids: &mut Vec<PaneId>) {
        match self {
            Layout::Pane(id) => ids.push(*id),
            Layout::Split { children, .. } => {
                for (child, _) in children {
                    child.collect_pane_ids(ids);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: u64) -> Layout {
        Layout::Pane(PaneId(id))
    }

    #[test]
    fn split_single_pane() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        assert_eq!(
            layout,
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![(pane(0), 0.5), (pane(1), 0.5)],
            }
        );
    }

    #[test]
    fn split_along_same_axis_adds_sibling() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(2))
            .unwrap();
        assert_eq!(
            layout,
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![(pane(0), 0.25), (pane(2), 0.25), (pane(1), 0.5)],
            }
        );
    }

    #[test]
    fn split_along_other_axis_nests() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(1), Axis::Vertical, PaneId(2)).unwrap();
        assert_eq!(
            layout,
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![
                    (pane(0), 0.5),
                    (
                        Layout::Split {
                            axis: Axis::Vertical,
                            children: vec![(pane(1), 0.5), (pane(2), 0.5)],
                        },
                        0.5
                    ),
                ],
            }
        );
    }

    #[test]
    fn split_in_nested_split_along_same_axis_adds_sibling() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(1), Axis::Vertical, PaneId(2)).unwrap();
        layout.split(PaneId(2), Axis::Vertical, PaneId(3)).unwrap();
        assert_eq!(
            layout,
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![
                    (pane(0), 0.5),
                    (
                        Layout::Split {
                            axis: Axis::Vertical,
                            children: vec![(pane(1), 0.5), (pane(2), 0.25), (pane(3), 0.25)],
                        },
                        0.5
                    ),
                ],
            }
        );
    }

    #[test]
    fn split_missing_pane_fails_without_change() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        let before = layout.clone();
        assert!(layout.split(PaneId(9), Axis::Vertical, PaneId(2)).is_err());
        assert_eq!(layout, before);
    }

    #[test]
    fn pane_ids_single() {
        assert_eq!(pane(0).pane_ids(), vec![PaneId(0)]);
    }

    #[test]
    fn pane_ids_in_order() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(0), Axis::Vertical, PaneId(2)).unwrap();
        layout
            .split(PaneId(1), Axis::Horizontal, PaneId(3))
            .unwrap();
        assert_eq!(
            layout.pane_ids(),
            vec![PaneId(0), PaneId(2), PaneId(1), PaneId(3)]
        );
    }
}
