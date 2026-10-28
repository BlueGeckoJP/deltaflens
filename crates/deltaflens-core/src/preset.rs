use crate::{Axis, Layout, PaneId};

/// A layout that can be made in one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// One pane.
    Single,
    /// Two panes side by side.
    TwoColumns,
    /// Three panes side by side.
    ThreeColumns,
    /// Four panes: top left, top right, bottom left, bottom right.
    Grid,
    /// An explorer on the left and a viewer on the right.
    ExplorerAndViewer,
}

/// The share of the width the explorer takes in [`Preset::ExplorerAndViewer`].
const EXPLORER_RATIO: f32 = 0.4;

impl Preset {
    /// The number of panes.
    #[must_use]
    pub fn pane_count(self) -> usize {
        match self {
            Preset::Single => 1,
            Preset::TwoColumns | Preset::ExplorerAndViewer => 2,
            Preset::ThreeColumns => 3,
            Preset::Grid => 4,
        }
    }

    /// The layout with `ids` in reading order, left to right and top to
    /// bottom. `ids` must have [`Self::pane_count`] IDs.
    pub(crate) fn layout(self, ids: &[PaneId]) -> Layout {
        let pane = |index: usize| Layout::Pane(ids[index]);
        let row = |ids: &[PaneId]| {
            // A preset has only a few panes.
            #[expect(clippy::cast_precision_loss)]
            let ratio = 1. / ids.len() as f32;
            Layout::Split {
                axis: Axis::Horizontal,
                children: ids.iter().map(|id| (Layout::Pane(*id), ratio)).collect(),
            }
        };
        match self {
            Preset::Single => pane(0),
            Preset::TwoColumns | Preset::ThreeColumns => row(ids),
            Preset::Grid => Layout::Split {
                axis: Axis::Vertical,
                children: vec![(row(&ids[..2]), 0.5), (row(&ids[2..]), 0.5)],
            },
            Preset::ExplorerAndViewer => Layout::Split {
                axis: Axis::Horizontal,
                children: vec![(pane(0), EXPLORER_RATIO), (pane(1), 1. - EXPLORER_RATIO)],
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(count: u64) -> Vec<PaneId> {
        (0..count).map(PaneId).collect()
    }

    #[test]
    fn layouts_have_their_pane_count_in_reading_order() {
        for preset in [
            Preset::Single,
            Preset::TwoColumns,
            Preset::ThreeColumns,
            Preset::Grid,
            Preset::ExplorerAndViewer,
        ] {
            let ids = ids(preset.pane_count() as u64);
            assert_eq!(preset.layout(&ids).pane_ids(), ids, "{preset:?}");
        }
    }

    #[test]
    fn single_is_one_pane() {
        assert_eq!(Preset::Single.layout(&ids(1)), Layout::Pane(PaneId(0)));
    }

    #[test]
    fn three_columns_are_equal() {
        let third = 1. / 3.;
        assert_eq!(
            Preset::ThreeColumns.layout(&ids(3)),
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![
                    (Layout::Pane(PaneId(0)), third),
                    (Layout::Pane(PaneId(1)), third),
                    (Layout::Pane(PaneId(2)), third),
                ],
            }
        );
    }

    #[test]
    fn grid_is_two_rows_of_two() {
        let row = |a, b| Layout::Split {
            axis: Axis::Horizontal,
            children: vec![
                (Layout::Pane(PaneId(a)), 0.5),
                (Layout::Pane(PaneId(b)), 0.5),
            ],
        };
        assert_eq!(
            Preset::Grid.layout(&ids(4)),
            Layout::Split {
                axis: Axis::Vertical,
                children: vec![(row(0, 1), 0.5), (row(2, 3), 0.5)],
            }
        );
    }

    #[test]
    fn explorer_and_viewer_gives_the_viewer_more_room() {
        assert_eq!(
            Preset::ExplorerAndViewer.layout(&ids(2)),
            Layout::Split {
                axis: Axis::Horizontal,
                children: vec![
                    (Layout::Pane(PaneId(0)), EXPLORER_RATIO),
                    (Layout::Pane(PaneId(1)), 1. - EXPLORER_RATIO),
                ],
            }
        );
    }
}
