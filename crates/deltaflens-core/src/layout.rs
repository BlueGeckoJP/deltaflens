use eyre::{Result, eyre};

use crate::{PaneId, Point, Rect, Size};

/// The direction in which the children of a split are laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children are placed side by side, left to right.
    Horizontal,
    /// Children are stacked, top to bottom.
    Vertical,
}

/// A direction from a pane to its neighbor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Up,
    Down,
}

/// How close two edges must be, in the unit square, to count as touching.
const EDGE_TOLERANCE: f32 = 1e-4;

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

    /// Removes the pane. The sibling right before it (or after it, if it was
    /// the first) takes its ratio. A split left with a single child is replaced
    /// by that child, and a split inside a split along the same axis is merged
    /// into it.
    ///
    /// Returns the pane closest to the removed one in the sibling that took
    /// its place.
    ///
    /// # Errors
    ///
    /// Returns an error if the pane is not in the layout, or if it is the only
    /// pane.
    pub fn remove(&mut self, target: PaneId) -> Result<PaneId> {
        if *self == Layout::Pane(target) {
            return Err(eyre!("pane {target:?} is the only pane"));
        }
        let neighbor = self
            .remove_inner(target)
            .ok_or_else(|| eyre!("pane {target:?} is not in the layout"))?;
        self.normalize();
        Ok(neighbor)
    }

    fn remove_inner(&mut self, target: PaneId) -> Option<PaneId> {
        let Layout::Split { children, .. } = self else {
            return None;
        };
        if let Some(index) = children
            .iter()
            .position(|(child, _)| *child == Layout::Pane(target))
        {
            let (_, ratio) = children.remove(index);
            // The sibling that was before the removed pane, or else after it.
            let (sibling, from_start) = if index > 0 {
                (index - 1, false)
            } else {
                (0, true)
            };
            let (child, sibling_ratio) = children.get_mut(sibling)?;
            *sibling_ratio += ratio;
            let ids = child.pane_ids();
            return if from_start {
                ids.first().copied()
            } else {
                ids.last().copied()
            };
        }
        children
            .iter_mut()
            .find_map(|(child, _)| child.remove_inner(target))
    }

    /// Replaces splits with a single child by that child, and merges splits
    /// into a parent split along the same axis.
    fn normalize(&mut self) {
        let Layout::Split { axis, children } = self else {
            return;
        };
        let axis = *axis;
        let mut merged = Vec::with_capacity(children.len());
        for (mut child, ratio) in std::mem::take(children) {
            child.normalize();
            match child {
                Layout::Split {
                    axis: child_axis,
                    children: grandchildren,
                } if child_axis == axis => {
                    merged.extend(
                        grandchildren
                            .into_iter()
                            .map(|(grandchild, inner)| (grandchild, inner * ratio)),
                    );
                }
                child => merged.push((child, ratio)),
            }
        }
        if merged.len() == 1 {
            let (only, _) = merged.remove(0);
            *self = only;
        } else {
            *children = merged;
        }
    }

    /// Returns where each pane is, in a unit square that stands for the whole
    /// layout.
    #[must_use]
    pub fn pane_rects(&self) -> Vec<(PaneId, Rect)> {
        let mut rects = Vec::new();
        let unit = Rect {
            origin: Point { x: 0., y: 0. },
            size: Size {
                width: 1.,
                height: 1.,
            },
        };
        self.collect_pane_rects(unit, &mut rects);
        rects
    }

    fn collect_pane_rects(&self, rect: Rect, rects: &mut Vec<(PaneId, Rect)>) {
        match self {
            Layout::Pane(id) => rects.push((*id, rect)),
            Layout::Split { axis, children } => {
                let mut offset = 0.;
                for (child, ratio) in children {
                    let child_rect = match axis {
                        Axis::Horizontal => Rect {
                            origin: Point {
                                x: rect.origin.x + offset * rect.size.width,
                                y: rect.origin.y,
                            },
                            size: Size {
                                width: ratio * rect.size.width,
                                height: rect.size.height,
                            },
                        },
                        Axis::Vertical => Rect {
                            origin: Point {
                                x: rect.origin.x,
                                y: rect.origin.y + offset * rect.size.height,
                            },
                            size: Size {
                                width: rect.size.width,
                                height: ratio * rect.size.height,
                            },
                        },
                    };
                    child.collect_pane_rects(child_rect, rects);
                    offset += ratio;
                }
            }
        }
    }

    /// Returns the pane next to `target` on `side`: of the panes that touch
    /// that edge, the one that shares the longest part of it, and the topmost
    /// (leftmost) of those.
    #[must_use]
    pub fn neighbor(&self, target: PaneId, side: Side) -> Option<PaneId> {
        let rects = self.pane_rects();
        let (_, from) = rects.iter().find(|(id, _)| *id == target)?;
        let from = *from;
        let mut best: Option<(PaneId, f32, f32)> = None;
        for (id, rect) in &rects {
            if *id == target {
                continue;
            }
            let (touches, overlap, start) = match side {
                Side::Left => (
                    (right(rect) - from.origin.x).abs() < EDGE_TOLERANCE,
                    overlap(rect.origin.y, bottom(rect), from.origin.y, bottom(&from)),
                    rect.origin.y,
                ),
                Side::Right => (
                    (rect.origin.x - right(&from)).abs() < EDGE_TOLERANCE,
                    overlap(rect.origin.y, bottom(rect), from.origin.y, bottom(&from)),
                    rect.origin.y,
                ),
                Side::Up => (
                    (bottom(rect) - from.origin.y).abs() < EDGE_TOLERANCE,
                    overlap(rect.origin.x, right(rect), from.origin.x, right(&from)),
                    rect.origin.x,
                ),
                Side::Down => (
                    (rect.origin.y - bottom(&from)).abs() < EDGE_TOLERANCE,
                    overlap(rect.origin.x, right(rect), from.origin.x, right(&from)),
                    rect.origin.x,
                ),
            };
            if !touches || overlap < EDGE_TOLERANCE {
                continue;
            }
            let better = best.is_none_or(|(_, best_overlap, best_start)| {
                overlap > best_overlap + EDGE_TOLERANCE
                    || ((overlap - best_overlap).abs() <= EDGE_TOLERANCE && start < best_start)
            });
            if better {
                best = Some((*id, overlap, start));
            }
        }
        best.map(|(id, _, _)| id)
    }

    /// The split reached by following `path`, the indices of the children
    /// from the root.
    fn node_mut(&mut self, path: &[usize]) -> Option<&mut Layout> {
        let Some((first, rest)) = path.split_first() else {
            return Some(self);
        };
        match self {
            Layout::Pane(_) => None,
            Layout::Split { children, .. } => children.get_mut(*first)?.0.node_mut(rest),
        }
    }

    /// The smallest size of the layout along `axis`, in the same unit as
    /// `min_pane`, with which every pane keeps at least `min_pane`.
    ///
    /// The children of a split grow and shrink with it, so a child with a small
    /// ratio needs the split to be large.
    #[must_use]
    pub fn min_extent(&self, axis: Axis, min_pane: f32) -> f32 {
        match self {
            Layout::Pane(_) => min_pane,
            Layout::Split {
                axis: split_axis,
                children,
            } if *split_axis == axis => children
                .iter()
                .map(|(child, ratio)| child.min_extent(axis, min_pane) / ratio.max(f32::EPSILON))
                .fold(0., f32::max),
            Layout::Split { children, .. } => children
                .iter()
                .map(|(child, _)| child.min_extent(axis, min_pane))
                .fold(0., f32::max),
        }
    }

    /// Moves the boundary after child `boundary` of the split at `path` to
    /// `offset` from the start of the split, which is `extent` long along its
    /// axis. Only the two children next to the boundary change. Each keeps the
    /// size that its panes need to be at least `min_pane`; if the two cannot
    /// both keep it, nothing changes.
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
        let Some(Layout::Split { axis, children }) = self.node_mut(path) else {
            return Err(eyre!("no split at {path:?}"));
        };
        let axis = *axis;
        if boundary + 1 >= children.len() {
            return Err(eyre!("no boundary after child {boundary}"));
        }
        if extent <= 0. {
            return Ok(());
        }
        let start: f32 = children[..boundary].iter().map(|(_, ratio)| ratio).sum();
        let pair = children[boundary].1 + children[boundary + 1].1;
        let min_before = children[boundary].0.min_extent(axis, min_pane) / extent;
        let min_after = children[boundary + 1].0.min_extent(axis, min_pane) / extent;
        if min_before + min_after > pair {
            return Ok(());
        }
        let before = (offset / extent - start).clamp(min_before, pair - min_after);
        children[boundary].1 = before;
        children[boundary + 1].1 = pair - before;
        Ok(())
    }

    /// Gives every child of the split at `path` the same ratio.
    ///
    /// # Errors
    ///
    /// Returns an error if there is no split at `path`.
    pub fn equalize(&mut self, path: &[usize]) -> Result<()> {
        let Some(Layout::Split { children, .. }) = self.node_mut(path) else {
            return Err(eyre!("no split at {path:?}"));
        };
        // A split has only a few children.
        #[expect(clippy::cast_precision_loss)]
        let ratio = 1. / children.len() as f32;
        for (_, child_ratio) in children {
            *child_ratio = ratio;
        }
        Ok(())
    }

    /// Returns the IDs of the panes that touch the top edge of the layout, from
    /// left to right.
    #[must_use]
    pub fn top_pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.collect_top_pane_ids(&mut ids);
        ids
    }

    fn collect_top_pane_ids(&self, ids: &mut Vec<PaneId>) {
        match self {
            Layout::Pane(id) => ids.push(*id),
            Layout::Split {
                axis: Axis::Horizontal,
                children,
            } => {
                for (child, _) in children {
                    child.collect_top_pane_ids(ids);
                }
            }
            Layout::Split {
                axis: Axis::Vertical,
                children,
            } => {
                if let Some((first, _)) = children.first() {
                    first.collect_top_pane_ids(ids);
                }
            }
        }
    }

    /// Returns the ID of the pane at the top right corner of the layout.
    ///
    /// Returns `None` only for a split without children.
    #[must_use]
    pub fn top_right_pane_id(&self) -> Option<PaneId> {
        match self {
            Layout::Pane(id) => Some(*id),
            Layout::Split {
                axis: Axis::Horizontal,
                children,
            } => children.last()?.0.top_right_pane_id(),
            Layout::Split {
                axis: Axis::Vertical,
                children,
            } => children.first()?.0.top_right_pane_id(),
        }
    }
}

fn right(rect: &Rect) -> f32 {
    rect.origin.x + rect.size.width
}

fn bottom(rect: &Rect) -> f32 {
    rect.origin.y + rect.size.height
}

/// The length of the overlap of two ranges.
fn overlap(start_a: f32, end_a: f32, start_b: f32, end_b: f32) -> f32 {
    (end_a.min(end_b) - start_a.max(start_b)).max(0.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(id: u64) -> Layout {
        Layout::Pane(PaneId(id))
    }

    fn split(axis: Axis, children: Vec<(Layout, f32)>) -> Layout {
        Layout::Split { axis, children }
    }

    /// `0 | 1` with `1` split into `1 / 2`.
    fn right_side_stacked() -> Layout {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(1), Axis::Vertical, PaneId(2)).unwrap();
        layout
    }

    #[test]
    fn remove_from_two_panes_leaves_one() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        assert_eq!(layout.remove(PaneId(1)).unwrap(), PaneId(0));
        assert_eq!(layout, pane(0));
    }

    #[test]
    fn remove_first_pane_gives_its_ratio_to_the_next() {
        let mut layout = split(
            Axis::Horizontal,
            vec![(pane(0), 0.2), (pane(1), 0.3), (pane(2), 0.5)],
        );
        assert_eq!(layout.remove(PaneId(0)).unwrap(), PaneId(1));
        assert_eq!(
            layout,
            split(Axis::Horizontal, vec![(pane(1), 0.5), (pane(2), 0.5)])
        );
    }

    #[test]
    fn remove_gives_its_ratio_to_the_previous() {
        let mut layout = split(
            Axis::Horizontal,
            vec![(pane(0), 0.2), (pane(1), 0.3), (pane(2), 0.5)],
        );
        assert_eq!(layout.remove(PaneId(2)).unwrap(), PaneId(1));
        assert_eq!(
            layout,
            split(Axis::Horizontal, vec![(pane(0), 0.2), (pane(1), 0.8)])
        );
    }

    #[test]
    fn remove_collapses_nested_split() {
        let mut layout = right_side_stacked();
        assert_eq!(layout.remove(PaneId(2)).unwrap(), PaneId(1));
        assert_eq!(
            layout,
            split(Axis::Horizontal, vec![(pane(0), 0.5), (pane(1), 0.5)])
        );
    }

    #[test]
    fn remove_merges_split_along_the_same_axis() {
        // 0 | (1 | 2) / 3: removing 3 leaves 0 | 1 | 2.
        let mut layout = split(
            Axis::Horizontal,
            vec![
                (pane(0), 0.5),
                (
                    split(
                        Axis::Vertical,
                        vec![
                            (
                                split(Axis::Horizontal, vec![(pane(1), 0.5), (pane(2), 0.5)]),
                                0.5,
                            ),
                            (pane(3), 0.5),
                        ],
                    ),
                    0.5,
                ),
            ],
        );
        assert_eq!(layout.remove(PaneId(3)).unwrap(), PaneId(2));
        assert_eq!(
            layout,
            split(
                Axis::Horizontal,
                vec![(pane(0), 0.5), (pane(1), 0.25), (pane(2), 0.25)]
            )
        );
    }

    #[test]
    fn remove_neighbor_is_closest_pane_of_the_sibling() {
        // (0 / 1) | 2: removing 2 gives its place to the split, and the pane
        // closest to it is the last one, 1.
        let mut layout = split(
            Axis::Horizontal,
            vec![
                (
                    split(Axis::Vertical, vec![(pane(0), 0.5), (pane(1), 0.5)]),
                    0.5,
                ),
                (pane(2), 0.5),
            ],
        );
        assert_eq!(layout.remove(PaneId(2)).unwrap(), PaneId(1));
        assert_eq!(
            layout,
            split(Axis::Vertical, vec![(pane(0), 0.5), (pane(1), 0.5)])
        );
    }

    #[test]
    fn remove_only_pane_fails() {
        let mut layout = pane(0);
        assert!(layout.remove(PaneId(0)).is_err());
        assert_eq!(layout, pane(0));
    }

    #[test]
    fn remove_missing_pane_fails_without_change() {
        let mut layout = right_side_stacked();
        let before = layout.clone();
        assert!(layout.remove(PaneId(9)).is_err());
        assert_eq!(layout, before);
    }

    #[test]
    fn pane_rects_follow_the_ratios() {
        let rects = right_side_stacked().pane_rects();
        let rect = |x, y, width, height| Rect {
            origin: Point { x, y },
            size: Size { width, height },
        };
        assert_eq!(
            rects,
            vec![
                (PaneId(0), rect(0., 0., 0.5, 1.)),
                (PaneId(1), rect(0.5, 0., 0.5, 0.5)),
                (PaneId(2), rect(0.5, 0.5, 0.5, 0.5)),
            ]
        );
    }

    #[test]
    fn neighbors_of_single_pane() {
        for side in [Side::Left, Side::Right, Side::Up, Side::Down] {
            assert_eq!(pane(0).neighbor(PaneId(0), side), None);
        }
    }

    #[test]
    fn neighbors_side_by_side() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        assert_eq!(layout.neighbor(PaneId(0), Side::Right), Some(PaneId(1)));
        assert_eq!(layout.neighbor(PaneId(1), Side::Left), Some(PaneId(0)));
        assert_eq!(layout.neighbor(PaneId(0), Side::Left), None);
        assert_eq!(layout.neighbor(PaneId(0), Side::Down), None);
    }

    #[test]
    fn neighbors_across_nested_splits() {
        let layout = right_side_stacked();
        assert_eq!(layout.neighbor(PaneId(1), Side::Down), Some(PaneId(2)));
        assert_eq!(layout.neighbor(PaneId(2), Side::Up), Some(PaneId(1)));
        assert_eq!(layout.neighbor(PaneId(2), Side::Left), Some(PaneId(0)));
        // Both 1 and 2 touch the right edge of 0 equally; the top one wins.
        assert_eq!(layout.neighbor(PaneId(0), Side::Right), Some(PaneId(1)));
    }

    #[test]
    fn neighbor_shares_the_longest_edge() {
        // 0 | 1 with 1 split 0.3 / 0.7: going right from 0 picks 2.
        let layout = split(
            Axis::Horizontal,
            vec![
                (pane(0), 0.5),
                (
                    split(Axis::Vertical, vec![(pane(1), 0.3), (pane(2), 0.7)]),
                    0.5,
                ),
            ],
        );
        assert_eq!(layout.neighbor(PaneId(0), Side::Right), Some(PaneId(2)));
    }

    fn ratios(layout: &Layout) -> Vec<f32> {
        match layout {
            Layout::Pane(_) => Vec::new(),
            Layout::Split { children, .. } => children.iter().map(|(_, ratio)| *ratio).collect(),
        }
    }

    fn assert_ratios(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len(), "{actual:?}");
        for (actual, expected) in actual.iter().zip(expected) {
            assert!((actual - expected).abs() < 1e-5, "{actual} != {expected}");
        }
    }

    #[test]
    fn min_extent_of_nested_splits() {
        // 0 | 1 with 1 split 1 / 2: across, both columns need 100; along the
        // stacked side, the column needs only one pane's worth.
        let layout = right_side_stacked();
        assert!((layout.min_extent(Axis::Horizontal, 100.) - 200.).abs() < 1e-3);
        assert!((layout.min_extent(Axis::Vertical, 100.) - 200.).abs() < 1e-3);
        let column = split(Axis::Vertical, vec![(pane(1), 0.25), (pane(2), 0.75)]);
        assert!((column.min_extent(Axis::Vertical, 100.) - 400.).abs() < 1e-3);
        assert!((column.min_extent(Axis::Horizontal, 100.) - 100.).abs() < 1e-3);
    }

    #[test]
    fn move_boundary_changes_the_two_neighbors() {
        let mut layout = split(
            Axis::Horizontal,
            vec![(pane(0), 0.25), (pane(1), 0.25), (pane(2), 0.5)],
        );
        layout.move_boundary(&[], 0, 400., 1000., 50.).unwrap();
        assert_ratios(&ratios(&layout), &[0.4, 0.1, 0.5]);
    }

    #[test]
    fn move_boundary_keeps_the_minimum_size() {
        let mut layout = split(Axis::Horizontal, vec![(pane(0), 0.5), (pane(1), 0.5)]);
        layout.move_boundary(&[], 0, 990., 1000., 100.).unwrap();
        assert_ratios(&ratios(&layout), &[0.9, 0.1]);
        layout.move_boundary(&[], 0, -50., 1000., 100.).unwrap();
        assert_ratios(&ratios(&layout), &[0.1, 0.9]);
    }

    #[test]
    fn move_boundary_keeps_the_minimum_of_nested_panes() {
        // The right column is a vertical split, which needs only one pane's
        // worth across.
        let mut layout = right_side_stacked();
        layout.move_boundary(&[], 0, 950., 1000., 100.).unwrap();
        assert_ratios(&ratios(&layout), &[0.9, 0.1]);
    }

    #[test]
    fn move_boundary_in_a_nested_split() {
        let mut layout = right_side_stacked();
        layout.move_boundary(&[1], 0, 300., 1000., 100.).unwrap();
        let Layout::Split { children, .. } = &layout else {
            panic!("not a split");
        };
        assert_ratios(&ratios(&children[1].0), &[0.3, 0.7]);
        assert_ratios(&ratios(&layout), &[0.5, 0.5]);
    }

    #[test]
    fn move_boundary_without_room_changes_nothing() {
        let mut layout = split(Axis::Horizontal, vec![(pane(0), 0.5), (pane(1), 0.5)]);
        layout.move_boundary(&[], 0, 100., 150., 100.).unwrap();
        assert_ratios(&ratios(&layout), &[0.5, 0.5]);
    }

    #[test]
    fn move_missing_boundary_fails() {
        let mut layout = split(Axis::Horizontal, vec![(pane(0), 0.5), (pane(1), 0.5)]);
        assert!(layout.move_boundary(&[], 1, 100., 1000., 10.).is_err());
        assert!(layout.move_boundary(&[0], 0, 100., 1000., 10.).is_err());
        assert!(pane(0).move_boundary(&[], 0, 100., 1000., 10.).is_err());
    }

    #[test]
    fn equalize_gives_every_child_the_same_ratio() {
        let mut layout = split(
            Axis::Horizontal,
            vec![(pane(0), 0.2), (pane(1), 0.3), (pane(2), 0.5)],
        );
        layout.equalize(&[]).unwrap();
        assert_ratios(&ratios(&layout), &[1. / 3., 1. / 3., 1. / 3.]);
    }

    #[test]
    fn equalize_nested_split_leaves_the_parent() {
        let mut layout = split(
            Axis::Horizontal,
            vec![
                (pane(0), 0.7),
                (
                    split(Axis::Vertical, vec![(pane(1), 0.2), (pane(2), 0.8)]),
                    0.3,
                ),
            ],
        );
        layout.equalize(&[1]).unwrap();
        let Layout::Split { children, .. } = &layout else {
            panic!("not a split");
        };
        assert_ratios(&ratios(&children[1].0), &[0.5, 0.5]);
        assert_ratios(&ratios(&layout), &[0.7, 0.3]);
    }

    #[test]
    fn equalize_pane_fails() {
        assert!(pane(0).equalize(&[]).is_err());
    }

    #[test]
    fn neighbor_of_missing_pane_is_none() {
        assert_eq!(right_side_stacked().neighbor(PaneId(9), Side::Left), None);
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

    #[test]
    fn top_panes_of_single_pane() {
        assert_eq!(pane(0).top_pane_ids(), vec![PaneId(0)]);
        assert_eq!(pane(0).top_right_pane_id(), Some(PaneId(0)));
    }

    #[test]
    fn top_panes_side_by_side() {
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        assert_eq!(layout.top_pane_ids(), vec![PaneId(0), PaneId(1)]);
        assert_eq!(layout.top_right_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn top_panes_stacked() {
        let mut layout = pane(0);
        layout.split(PaneId(0), Axis::Vertical, PaneId(1)).unwrap();
        assert_eq!(layout.top_pane_ids(), vec![PaneId(0)]);
        assert_eq!(layout.top_right_pane_id(), Some(PaneId(0)));
    }

    #[test]
    fn top_panes_with_right_side_stacked() {
        // 0 | 1
        //   | -
        //   | 2
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(1), Axis::Vertical, PaneId(2)).unwrap();
        assert_eq!(layout.top_pane_ids(), vec![PaneId(0), PaneId(1)]);
        assert_eq!(layout.top_right_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn top_panes_with_top_side_by_side() {
        // 0 | 1
        // -----
        //   2
        let mut layout = pane(0);
        layout.split(PaneId(0), Axis::Vertical, PaneId(2)).unwrap();
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        assert_eq!(layout.top_pane_ids(), vec![PaneId(0), PaneId(1)]);
        assert_eq!(layout.top_right_pane_id(), Some(PaneId(1)));
    }

    #[test]
    fn top_panes_deeply_nested() {
        // 0 | 1 | 3
        //   | ----
        //   |  2
        let mut layout = pane(0);
        layout
            .split(PaneId(0), Axis::Horizontal, PaneId(1))
            .unwrap();
        layout.split(PaneId(1), Axis::Vertical, PaneId(2)).unwrap();
        layout
            .split(PaneId(1), Axis::Horizontal, PaneId(3))
            .unwrap();
        assert_eq!(layout.top_pane_ids(), vec![PaneId(0), PaneId(1), PaneId(3)]);
        assert_eq!(layout.top_right_pane_id(), Some(PaneId(3)));
    }

    #[test]
    fn top_right_pane_of_empty_split() {
        let layout = Layout::Split {
            axis: Axis::Horizontal,
            children: Vec::new(),
        };
        assert_eq!(layout.top_pane_ids(), Vec::new());
        assert_eq!(layout.top_right_pane_id(), None);
    }
}
