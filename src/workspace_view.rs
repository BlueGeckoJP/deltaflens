use std::{collections::HashMap, path::PathBuf};

use deltaflens_core::{Axis, Direction, Layout, PaneId, Tab, TabContent, TabId, Workspace};
use eyre::Result;
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FocusHandle, MouseButton, MouseUpEvent,
    PathPromptOptions, Window, actions, div, prelude::*, px, relative, rgb,
};

use crate::viewer::Viewer;

actions!(
    deltaflens,
    [
        Open,
        NewTab,
        CloseTab,
        ActivateNextTab,
        ActivatePreviousTab,
        ResetView,
        PreviousImage,
        NextImage,
    ]
);

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
    pub fn open_in_new_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let pane_id = self.workspace.focused_pane();
        let content = TabContent::Viewer { path: path.clone() };
        match self.workspace.open_tab(pane_id, content) {
            Ok(tab_id) => {
                let viewer = cx.new(|cx| Viewer::new(path, window, cx));
                self.viewers.insert(tab_id, viewer);
                cx.notify();
            }
            Err(error) => tracing::error!("{error:#}"),
        }
    }

    /// Shows `path` in the active tab of the focused pane, or in a new tab if
    /// the pane has none.
    fn open_in_active_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab().map(Tab::id) {
            Some(tab_id) => {
                if let Err(error) = self.replace_tab_path(tab_id, path, window, cx) {
                    tracing::error!("{error:#}");
                }
            }
            None => self.open_in_new_tab(path, window, cx),
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
        Self::prompt_for_image(window, cx, Self::open_in_active_tab);
    }

    #[expect(clippy::unused_self, reason = "`cx.listener` takes a method")]
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        Self::prompt_for_image(window, cx, Self::open_in_new_tab);
    }

    /// Asks for an image with the file dialog and passes it to `open`, unless
    /// the dialog is cancelled.
    fn prompt_for_image(
        window: &mut Window,
        cx: &mut Context<Self>,
        open: impl FnOnce(&mut Self, PathBuf, &mut Window, &mut Context<Self>) + 'static,
    ) {
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
                let _ = this.update_in(cx, |this, window, cx| open(this, path, window, cx));
            }
        })
        .detach();
    }

    fn activate_tab(&mut self, pane_id: PaneId, tab_id: TabId, cx: &mut Context<Self>) {
        let result = self
            .workspace
            .focus_pane(pane_id)
            .and_then(|()| self.workspace.pane_mut(pane_id))
            .and_then(|pane| pane.activate_tab(tab_id));
        if let Err(error) = result {
            tracing::error!("{error:#}");
        }
        cx.notify();
    }

    /// Closes the tab and releases its viewer, with its decoded image.
    fn close_tab(&mut self, pane_id: PaneId, tab_id: TabId, cx: &mut Context<Self>) {
        let result = self
            .workspace
            .pane_mut(pane_id)
            .and_then(|pane| pane.close_tab(tab_id));
        match result {
            Ok(()) => {
                self.viewers.remove(&tab_id);
                cx.notify();
            }
            Err(error) => tracing::error!("{error:#}"),
        }
    }

    fn close_active_tab(&mut self, _: &CloseTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(tab_id) = self.active_tab().map(Tab::id) {
            self.close_tab(self.workspace.focused_pane(), tab_id, cx);
        }
    }

    fn activate_next_tab(&mut self, _: &ActivateNextTab, _: &mut Window, cx: &mut Context<Self>) {
        if let Ok(pane) = self.workspace.pane_mut(self.workspace.focused_pane()) {
            pane.activate_next_tab();
            cx.notify();
        }
    }

    fn activate_previous_tab(
        &mut self,
        _: &ActivatePreviousTab,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Ok(pane) = self.workspace.pane_mut(self.workspace.focused_pane()) {
            pane.activate_previous_tab();
            cx.notify();
        }
    }

    /// Shows the file name of the active tab in the window title.
    fn update_title(&mut self, window: &mut Window) {
        let title = match self.active_tab() {
            Some(tab) => format!("{} - {APP_NAME}", tab_name(tab)),
            None => APP_NAME.to_owned(),
        };
        if title != self.title {
            window.set_window_title(&title);
            self.title = title;
        }
    }

    fn render_layout(&self, layout: &Layout, cx: &mut Context<Self>) -> AnyElement {
        match layout {
            Layout::Pane(pane_id) => self.render_pane(*pane_id, cx),
            Layout::Split { axis, children } => div()
                .size_full()
                .flex()
                .when(*axis == Axis::Vertical, Styled::flex_col)
                .children(children.iter().map(|(child, ratio)| {
                    let cell = div().overflow_hidden().child(self.render_layout(child, cx));
                    match axis {
                        Axis::Horizontal => cell.h_full().w(relative(*ratio)),
                        Axis::Vertical => cell.w_full().h(relative(*ratio)),
                    }
                }))
                .into_any_element(),
        }
    }

    fn render_pane(&self, pane_id: PaneId, cx: &mut Context<Self>) -> AnyElement {
        let Ok(pane) = self.workspace.pane(pane_id) else {
            return div().into_any_element();
        };
        let active = pane.active_tab().map(Tab::id);
        let content = match active.and_then(|tab_id| self.viewers.get(&tab_id)) {
            Some(viewer) => viewer.clone().into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x80_80_80))
                .child("Ctrl+O: open an image")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .when(!pane.tabs().is_empty(), |this| {
                this.child(Self::render_tab_bar(pane_id, pane.tabs(), active, cx))
            })
            .child(div().flex_1().overflow_hidden().child(content))
            .into_any_element()
    }

    fn render_tab_bar(
        pane_id: PaneId,
        tabs: &[Tab],
        active: Option<TabId>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(ElementId::Name(format!("tab-bar-{pane_id:?}").into()))
            .flex()
            .flex_shrink_0()
            .w_full()
            .h(px(32.))
            .overflow_x_scroll()
            .bg(rgb(0x14_14_14))
            .text_sm()
            .children(tabs.iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id();
                let is_active = active == Some(tab_id);
                div()
                    .id(("tab", index))
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap_1()
                    .max_w(px(240.))
                    .h_full()
                    .pl_3()
                    .pr_1()
                    .border_r_1()
                    .border_color(rgb(0x2a_2a_2a))
                    .bg(rgb(if is_active { 0x1e_1e_1e } else { 0x14_14_14 }))
                    .text_color(rgb(if is_active { 0xe0_e0_e0 } else { 0x90_90_90 }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.activate_tab(pane_id, tab_id, cx);
                    }))
                    .on_mouse_up(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                            this.close_tab(pane_id, tab_id, cx);
                        }),
                    )
                    .child(div().min_w_0().truncate().child(tab_name(tab)))
                    .child(
                        div()
                            .id("close")
                            .flex_shrink_0()
                            .px_1()
                            .rounded_sm()
                            .hover(|style| style.bg(rgb(0x3a_3a_3a)))
                            .child("×")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                cx.stop_propagation();
                                this.close_tab(pane_id, tab_id, cx);
                            })),
                    )
            }))
    }
}

/// The label of a tab: the file name of what it shows.
fn tab_name(tab: &Tab) -> String {
    match tab.content() {
        TabContent::Viewer { path } => path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned(),
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
            .on_action(cx.listener(Self::new_tab))
            .on_action(cx.listener(Self::close_active_tab))
            .on_action(cx.listener(Self::activate_next_tab))
            .on_action(cx.listener(Self::activate_previous_tab))
            .on_action(cx.listener(Self::reset_view))
            .on_action(cx.listener(Self::previous_image))
            .on_action(cx.listener(Self::next_image))
            .child(self.render_layout(self.workspace.layout(), cx))
    }
}
