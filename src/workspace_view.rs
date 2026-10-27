use std::{collections::HashMap, path::PathBuf};

use deltaflens_core::{Axis, Direction, Layout, PaneId, Tab, TabContent, TabId, Workspace};
use eyre::Result;
use gpui::{
    AnyElement, Context, Entity, FocusHandle, PathPromptOptions, Window, actions, div, prelude::*,
    relative, rgb,
};

use crate::viewer::Viewer;

actions!(deltaflens, [Open, ResetView, PreviousImage, NextImage]);

pub const APP_NAME: &str = "deltaflens";

/// The content of a window: renders the core [`Workspace`] and keeps the UI
/// state of each tab.
pub struct WorkspaceView {
    workspace: Workspace,
    viewers: HashMap<TabId, Entity<Viewer>>,
    focus_handle: FocusHandle,
    /// The window title last set, to avoid setting it on every frame.
    title: String,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        Self {
            workspace: Workspace::new(),
            viewers: HashMap::new(),
            focus_handle,
            title: APP_NAME.to_owned(),
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

    /// Shows `path` in the active tab of the focused pane, or in a new tab if
    /// the pane has none.
    fn open_in_active_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let result = match self.active_tab().map(Tab::id) {
            Some(tab_id) => self.replace_tab_path(tab_id, path, window, cx),
            None => self.open_in_new_tab(path, window, cx),
        };
        if let Err(error) = result {
            tracing::error!("{error:#}");
        }
    }

    /// Shows `path` in a tab of the focused pane.
    fn replace_tab_path(
        &mut self,
        tab_id: TabId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let content = TabContent::Viewer { path: path.clone() };
        self.workspace
            .pane_mut(self.workspace.focused_pane())?
            .set_tab_content(tab_id, content)?;
        if let Some(viewer) = self.viewers.get(&tab_id) {
            viewer.update(cx, |viewer, cx| viewer.load(path, window, cx));
        }
        cx.notify();
        Ok(())
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.workspace
            .pane(self.workspace.focused_pane())
            .ok()?
            .active_tab()
    }

    fn active_viewer(&self) -> Option<&Entity<Viewer>> {
        self.viewers.get(&self.active_tab()?.id())
    }

    fn reset_view(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.active_viewer() {
            viewer.update(cx, Viewer::reset_view);
        }
    }

    fn previous_image(&mut self, _: &PreviousImage, window: &mut Window, cx: &mut Context<Self>) {
        self.move_in_folder(Direction::Previous, window, cx);
    }

    fn next_image(&mut self, _: &NextImage, window: &mut Window, cx: &mut Context<Self>) {
        self.move_in_folder(Direction::Next, window, cx);
    }

    /// Shows the previous or next image of the folder in the active tab.
    fn move_in_folder(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(tab_id) = self.active_tab().map(Tab::id) else {
            return;
        };
        let Some(path) = self
            .viewers
            .get(&tab_id)
            .and_then(|viewer| viewer.read(cx).neighbor(direction))
        else {
            return;
        };
        if let Err(error) = self.replace_tab_path(tab_id, path, window, cx) {
            tracing::error!("{error:#}");
        }
    }

    #[expect(clippy::unused_self, reason = "`cx.listener` takes a method")]
    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn_in(window, async move |this, cx| {
            let path = match paths.await {
                Ok(Ok(paths)) => paths.and_then(|paths| paths.into_iter().next()),
                Ok(Err(error)) => {
                    tracing::error!("failed to show the file dialog: {error:#}");
                    None
                }
                // The dialog was dropped without an answer.
                Err(_) => None,
            };
            if let Some(path) = path {
                let _ = this.update_in(cx, |this, window, cx| {
                    this.open_in_active_tab(path, window, cx);
                });
            }
        })
        .detach();
    }

    /// Shows the file name of the active tab in the window title.
    fn update_title(&mut self, window: &mut Window) {
        let title = match self.active_tab().map(Tab::content) {
            Some(TabContent::Viewer { path }) => match path.file_name() {
                Some(name) => format!("{} - {APP_NAME}", name.to_string_lossy()),
                None => APP_NAME.to_owned(),
            },
            None => APP_NAME.to_owned(),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_title(window);
        div()
            .size_full()
            .bg(rgb(0x1e_1e_1e))
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::open))
            .on_action(cx.listener(Self::reset_view))
            .on_action(cx.listener(Self::previous_image))
            .on_action(cx.listener(Self::next_image))
            .child(self.render_layout(self.workspace.layout()))
    }
}
