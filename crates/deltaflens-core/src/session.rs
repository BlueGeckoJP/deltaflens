//! The layout and the tabs of a window, saved when they change and restored
//! when the application starts again.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use eyre::{Result, WrapErr, bail, eyre};
use serde::{Deserialize, Serialize};

use crate::{Axis, Layout, Pane, PaneId, Tab, TabContent, TabId, Workspace, platform};

/// The version of the format, changed when old files can no longer be read.
const VERSION: u32 = 1;

/// The saved state of a [`Workspace`]. The state of each tab beyond what it
/// shows, such as the zoom of a viewer, is not saved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Session {
    version: u32,
    /// The index of the focused pane, in the order of [`Layout::pane_ids`].
    focused: usize,
    layout: SavedLayout,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SavedLayout {
    Pane {
        tabs: Vec<SavedTab>,
        /// The index of the active tab.
        active: Option<usize>,
    },
    Split {
        axis: SavedAxis,
        children: Vec<SavedChild>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct SavedChild {
    ratio: f32,
    layout: SavedLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SavedAxis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum SavedTab {
    Viewer { path: String },
    Explorer { dir: String },
    Settings,
}

impl Session {
    /// The session file: `session.toml` in the state directory of the
    /// application.
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        Some(platform::state_dir()?.join("deltaflens/session.toml"))
    }

    /// The session of `workspace`. Tabs whose paths are not UTF-8 are left
    /// out, as the file cannot hold them.
    #[must_use]
    pub fn of(workspace: &Workspace) -> Self {
        let focused = workspace
            .layout()
            .pane_ids()
            .iter()
            .position(|&id| id == workspace.focused_pane())
            .unwrap_or(0);
        Self {
            version: VERSION,
            focused,
            layout: save_layout(workspace, workspace.layout()),
        }
    }

    /// Reads the session at `path`. `None` if there is none.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read or is not a session.
    pub fn load(path: &Path) -> Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).wrap_err_with(|| format!("failed to read {}", path.display()));
            }
        };
        Self::parse(&text)
            .map(Some)
            .wrap_err_with(|| format!("failed to read {}", path.display()))
    }

    fn parse(text: &str) -> Result<Self> {
        let session: Self = toml::from_str(text)?;
        if session.version != VERSION {
            bail!("unknown version {}", session.version);
        }
        Ok(session)
    }

    /// Writes the session to `path`, creating its folder if there is none.
    /// The file is replaced at once, so that a reader never sees it half
    /// written.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be written.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string(self)?;
        let dir = path
            .parent()
            .ok_or_else(|| eyre!("{} has no folder", path.display()))?;
        fs::create_dir_all(dir)?;
        let temporary = dir.join(".session.toml.new");
        fs::write(&temporary, text)?;
        fs::rename(&temporary, path).wrap_err_with(|| format!("failed to write {}", path.display()))
    }

    /// Makes the workspace of the session. Tabs for which `exists` is false,
    /// such as files that are gone, are left out, and so are the panes left
    /// without tabs, unless no pane has any.
    ///
    /// # Errors
    ///
    /// Returns an error if the session is not a valid layout, such as a split
    /// without children.
    pub fn restore(&self, exists: impl Fn(&TabContent) -> bool) -> Result<Workspace> {
        let mut workspace = Workspace::new();
        let mut panes = Vec::new();
        let layout = restore_layout(&self.layout, &exists, &mut workspace, &mut panes)?;
        let focused = layout
            .pane_ids()
            .get(self.focused)
            .copied()
            .unwrap_or_else(|| layout.pane_ids()[0]);
        workspace.replace(layout, panes, focused);
        workspace.close_empty_panes();
        Ok(workspace)
    }
}

fn save_layout(workspace: &Workspace, layout: &Layout) -> SavedLayout {
    match layout {
        Layout::Pane(id) => {
            let Ok(pane) = workspace.pane(*id) else {
                return SavedLayout::Pane {
                    tabs: Vec::new(),
                    active: None,
                };
            };
            let saved: Vec<(TabId, SavedTab)> = pane
                .tabs()
                .iter()
                .filter_map(|tab| Some((tab.id(), save_tab(tab.content())?)))
                .collect();
            let active = pane
                .active_tab()
                .and_then(|active| saved.iter().position(|(id, _)| *id == active.id()));
            SavedLayout::Pane {
                tabs: saved.into_iter().map(|(_, tab)| tab).collect(),
                active,
            }
        }
        Layout::Split { axis, children } => SavedLayout::Split {
            axis: match axis {
                Axis::Horizontal => SavedAxis::Horizontal,
                Axis::Vertical => SavedAxis::Vertical,
            },
            children: children
                .iter()
                .map(|(child, ratio)| SavedChild {
                    ratio: *ratio,
                    layout: save_layout(workspace, child),
                })
                .collect(),
        },
    }
}

fn save_tab(content: &TabContent) -> Option<SavedTab> {
    Some(match content {
        TabContent::Viewer { path } => SavedTab::Viewer {
            path: path.to_str()?.to_owned(),
        },
        TabContent::Explorer { dir } => SavedTab::Explorer {
            dir: dir.to_str()?.to_owned(),
        },
        TabContent::Settings => SavedTab::Settings,
    })
}

fn restore_layout(
    saved: &SavedLayout,
    exists: &impl Fn(&TabContent) -> bool,
    workspace: &mut Workspace,
    panes: &mut Vec<(PaneId, Pane)>,
) -> Result<Layout> {
    match saved {
        SavedLayout::Pane { tabs, active } => {
            let id = PaneId(workspace.allocate_id());
            let mut pane = Pane::default();
            let mut active_id = None;
            for (index, tab) in tabs.iter().enumerate() {
                let content = match tab {
                    SavedTab::Viewer { path } => TabContent::Viewer { path: path.into() },
                    SavedTab::Explorer { dir } => TabContent::Explorer { dir: dir.into() },
                    SavedTab::Settings => TabContent::Settings,
                };
                if !exists(&content) {
                    continue;
                }
                let tab_id = TabId(workspace.allocate_id());
                pane.add_tab(Tab::new(tab_id, content));
                if Some(index) <= *active {
                    // The active tab, or the one before it if it is gone.
                    active_id = Some(tab_id);
                }
            }
            // The first tab, if all up to the active one are gone.
            if let Some(tab_id) = active_id.or_else(|| pane.tabs().first().map(Tab::id)) {
                pane.activate_tab(tab_id)?;
            }
            panes.push((id, pane));
            Ok(Layout::Pane(id))
        }
        SavedLayout::Split { axis, children } => {
            let total: f32 = children.iter().map(|child| child.ratio).sum();
            // Also rejects NaN.
            if children.is_empty() || total.is_nan() || total <= 0.0 {
                bail!("a split without children");
            }
            let children = children
                .iter()
                .map(|child| {
                    if child.ratio.is_nan() || child.ratio <= 0.0 {
                        bail!("a split with a child of ratio {}", child.ratio);
                    }
                    // Saved ratios may not add up to 1.0 exactly.
                    let layout = restore_layout(&child.layout, exists, workspace, panes)?;
                    Ok((layout, child.ratio / total))
                })
                .collect::<Result<_>>()?;
            Ok(Layout::Split {
                axis: match axis {
                    SavedAxis::Horizontal => Axis::Horizontal,
                    SavedAxis::Vertical => Axis::Vertical,
                },
                children,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::TempDir;

    fn viewer(path: &str) -> TabContent {
        TabContent::Viewer { path: path.into() }
    }

    fn explorer(dir: &str) -> TabContent {
        TabContent::Explorer { dir: dir.into() }
    }

    fn contents(workspace: &Workspace) -> Vec<(Vec<TabContent>, Option<TabContent>)> {
        workspace
            .layout()
            .pane_ids()
            .into_iter()
            .map(|id| {
                let pane = workspace.pane(id).unwrap();
                (
                    pane.tabs()
                        .iter()
                        .map(|tab| tab.content().clone())
                        .collect(),
                    pane.active_tab().map(|tab| tab.content().clone()),
                )
            })
            .collect()
    }

    /// Two panes side by side, the right one focused: an explorer and two
    /// viewers on the left, with the first viewer active, and a copy of that
    /// viewer and the settings on the right.
    fn sample() -> Workspace {
        let mut workspace = Workspace::new();
        let left = workspace.focused_pane();
        workspace.open_tab(left, explorer("/pictures")).unwrap();
        let first = workspace.open_tab(left, viewer("/pictures/a.jxl")).unwrap();
        workspace.open_tab(left, viewer("/pictures/b.png")).unwrap();
        workspace
            .pane_mut(left)
            .unwrap()
            .activate_tab(first)
            .unwrap();
        // The new pane gets a copy of the active tab, and the focus.
        let right = workspace.split_pane(left, Axis::Horizontal).unwrap();
        workspace.open_tab(right, TabContent::Settings).unwrap();
        workspace
    }

    #[test]
    fn saved_session_restores_the_same_workspace() {
        let workspace = sample();
        let dir = TempDir::with_files("session-round-trip", &[]);
        let path = dir.path().join("deltaflens/session.toml");
        Session::of(&workspace).save(&path).unwrap();

        let restored = Session::load(&path)
            .unwrap()
            .unwrap()
            .restore(|_| true)
            .unwrap();
        assert_eq!(contents(&restored), contents(&workspace));
        let ratios = |workspace: &Workspace| match workspace.layout() {
            Layout::Split { axis, children } => (
                *axis,
                children.iter().map(|(_, ratio)| *ratio).collect::<Vec<_>>(),
            ),
            Layout::Pane(_) => panic!("not a split"),
        };
        assert_eq!(ratios(&restored), ratios(&workspace));
        let focused = |workspace: &Workspace| {
            workspace
                .layout()
                .pane_ids()
                .iter()
                .position(|&id| id == workspace.focused_pane())
        };
        assert_eq!(focused(&restored), focused(&workspace));
    }

    #[test]
    fn missing_session_is_none() {
        let dir = TempDir::with_files("session-missing", &[]);
        assert_eq!(
            Session::load(&dir.path().join("session.toml")).unwrap(),
            None
        );
    }

    #[test]
    fn broken_session_is_an_error() {
        let dir = TempDir::with_files("session-broken", &["session.toml"]);
        let path = dir.path().join("session.toml");
        fs::write(&path, "version = 1\nfocused = \"x\"\n").unwrap();
        assert!(Session::load(&path).is_err());
        fs::write(&path, "[").unwrap();
        assert!(Session::load(&path).is_err());
    }

    #[test]
    fn session_of_another_version_is_an_error() {
        let mut text = toml::to_string(&Session::of(&sample())).unwrap();
        text = text.replace("version = 1", "version = 99");
        assert!(Session::parse(&text).is_err());
    }

    #[test]
    fn gone_files_are_left_out() {
        let session = Session::of(&sample());
        let restored = session
            .restore(|content| *content != viewer("/pictures/a.jxl"))
            .unwrap();
        // The active tab is gone, so the one before it is active.
        assert_eq!(
            contents(&restored),
            [
                (
                    vec![explorer("/pictures"), viewer("/pictures/b.png")],
                    Some(explorer("/pictures"))
                ),
                (vec![TabContent::Settings], Some(TabContent::Settings)),
            ]
        );
    }

    #[test]
    fn panes_left_without_tabs_are_closed() {
        let session = Session::of(&sample());
        let restored = session
            .restore(|content| *content == TabContent::Settings)
            .unwrap();
        assert_eq!(restored.layout().pane_ids().len(), 1);
        assert_eq!(
            contents(&restored),
            [(vec![TabContent::Settings], Some(TabContent::Settings))]
        );
    }

    #[test]
    fn nothing_left_gives_one_empty_pane() {
        let restored = Session::of(&sample()).restore(|_| false).unwrap();
        assert_eq!(contents(&restored), [(vec![], None)]);
    }

    #[test]
    fn split_without_children_is_an_error() {
        let text = "version = 1\nfocused = 0\n\n[layout]\ntype = \"split\"\naxis = \"horizontal\"\nchildren = []\n";
        let session = Session::parse(text).unwrap();
        assert!(session.restore(|_| true).is_err());
    }
}
