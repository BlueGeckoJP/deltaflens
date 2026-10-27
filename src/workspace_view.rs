use std::{collections::HashMap, path::PathBuf};

use deltaflens_core::{Axis, Layout, PaneId, TabContent, TabId, Workspace};
use eyre::Result;
use gpui::{
    AnyElement, Context, Entity, FocusHandle, Window, actions, div, prelude::*, relative, rgb,
};

use crate::viewer::Viewer;

actions!(deltaflens, [ResetView]);

/// The content of a window: renders the core [`Workspace`] and keeps the UI
/// state of each tab.
pub struct WorkspaceView {
    workspace: Workspace,
    viewers: HashMap<TabId, Entity<Viewer>>,
    focus_handle: FocusHandle,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            workspace: Workspace::new(),
            viewers: HashMap::new(),
            focus_handle,
        }
    }

    /// Opens `path` in a new tab of the focused pane.
    pub fn open_in_new_tab(
        &mut self,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let pane_id = self.workspace.focused_pane();
        let tab_id = self
            .workspace
            .open_tab(pane_id, TabContent::Viewer { path: path.clone() })?;
        let viewer = cx.new(|cx| Viewer::new(path, window, cx));
        self.viewers.insert(tab_id, viewer);
        cx.notify();
        Ok(())
    }

    fn active_viewer(&self) -> Option<&Entity<Viewer>> {
        let pane = self.workspace.pane(self.workspace.focused_pane()).ok()?;
        self.viewers.get(&pane.active_tab()?.id())
    }

    fn reset_view(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.active_viewer() {
            viewer.update(cx, Viewer::reset_view);
        }
    }

    fn render_layout(&self, layout: &Layout) -> AnyElement {
        match layout {
            Layout::Pane(pane_id) => self.render_pane(*pane_id),
            Layout::Split { axis, children } => div()
                .size_full()
                .flex()
                .when(*axis == Axis::Vertical, Styled::flex_col)
                .children(children.iter().map(|(child, ratio)| {
                    let cell = div().overflow_hidden().child(self.render_layout(child));
                    match axis {
                        Axis::Horizontal => cell.h_full().w(relative(*ratio)),
                        Axis::Vertical => cell.w_full().h(relative(*ratio)),
                    }
                }))
                .into_any_element(),
        }
    }

    fn render_pane(&self, pane_id: PaneId) -> AnyElement {
        let viewer = self
            .workspace
            .pane(pane_id)
            .ok()
            .and_then(|pane| pane.active_tab())
            .and_then(|tab| self.viewers.get(&tab.id()));
        div()
            .size_full()
            .when_some(viewer.cloned(), ParentElement::child)
            .into_any_element()
    }
}

impl Render for WorkspaceView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(rgb(0x1e_1e_1e))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::reset_view))
            .child(self.render_layout(self.workspace.layout()))
    }
}
