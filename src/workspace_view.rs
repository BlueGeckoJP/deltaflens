use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use deltaflens_core::{Axis, Direction, Layout, PaneId, Tab, TabContent, TabId, Workspace};
use eyre::{Result, eyre};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FocusHandle, MouseButton, MouseUpEvent,
    PathPromptOptions, Window, actions, div, prelude::*, px, relative, rgb,
};

use crate::{
    explorer::{Explorer, ExplorerEvent},
    thumbnails::Thumbnails,
    viewer::Viewer,
};

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
        GoBack,
        GoForward,
        GoUp,
    ]
);

pub const APP_NAME: &str = "deltaflens";

/// The UI state of a tab.
enum TabView {
    Viewer(Entity<Viewer>),
    Explorer(Entity<Explorer>),
}

impl TabView {
    fn to_element(&self) -> AnyElement {
        match self {
            Self::Viewer(viewer) => viewer.clone().into_any_element(),
            Self::Explorer(explorer) => explorer.clone().into_any_element(),
        }
    }
}

/// The content of a window: renders the core [`Workspace`] and keeps the UI
/// state of each tab.
pub struct WorkspaceView {
    workspace: Workspace,
    views: HashMap<TabId, TabView>,
    thumbnails: Entity<Thumbnails>,
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
            views: HashMap::new(),
            thumbnails: cx.new(Thumbnails::new),
            focus_handle,
            title: APP_NAME.to_owned(),
        }
    }

    /// Opens `path` in a new tab of the focused pane: an explorer for a folder,
    /// a viewer otherwise.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // The explorer moves up and shows the path bar from an absolute path.
        let path = std::path::absolute(&path).unwrap_or(path);
        let content = if path.is_dir() {
            TabContent::Explorer { dir: path }
        } else {
            TabContent::Viewer { path }
        };
        self.open_tab(content, window, cx);
    }

    /// Opens a new tab in the focused pane, to the right of its active tab.
    fn open_tab(&mut self, content: TabContent, window: &mut Window, cx: &mut Context<Self>) {
        let pane_id = self.workspace.focused_pane();
        let tab_id = match self.workspace.open_tab(pane_id, content.clone()) {
            Ok(tab_id) => tab_id,
            Err(error) => {
                tracing::error!("{error:#}");
                return;
            }
        };
        let view = match content {
            TabContent::Viewer { path } => {
                TabView::Viewer(cx.new(|cx| Viewer::new(path, window, cx)))
            }
            TabContent::Explorer { dir } => {
                let thumbnails = self.thumbnails.clone();
                let explorer = cx.new(|cx| Explorer::new(dir, thumbnails, cx));
                cx.subscribe_in(&explorer, window, move |this, _, event, window, cx| {
                    this.on_explorer_event(tab_id, event, window, cx);
                })
                .detach();
                TabView::Explorer(explorer)
            }
        };
        self.views.insert(tab_id, view);
        cx.notify();
    }

    fn on_explorer_event(
        &mut self,
        tab_id: TabId,
        event: &ExplorerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ExplorerEvent::Navigated(dir) => {
                let content = TabContent::Explorer { dir: dir.clone() };
                if let Err(error) = self.set_tab_content(tab_id, content) {
                    tracing::error!("{error:#}");
                }
                cx.notify();
            }
            ExplorerEvent::OpenImage(path) => {
                // Open next to the explorer, in its pane.
                if let Some(pane_id) = self.workspace.pane_of_tab(tab_id) {
                    let _ = self.workspace.focus_pane(pane_id);
                }
                let content = TabContent::Viewer { path: path.clone() };
                self.open_tab(content, window, cx);
            }
        }
    }

    fn set_tab_content(&mut self, tab_id: TabId, content: TabContent) -> Result<()> {
        let pane_id = self
            .workspace
            .pane_of_tab(tab_id)
            .ok_or_else(|| eyre!("tab {tab_id:?} is not open"))?;
        self.workspace
            .pane_mut(pane_id)?
            .set_tab_content(tab_id, content)
    }

    /// Shows `path` in the active tab of the focused pane if it is a viewer, or
    /// in a new tab otherwise.
    fn open_in_active_tab(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        match self.active_tab().map(Tab::id) {
            Some(tab_id) if matches!(self.views.get(&tab_id), Some(TabView::Viewer(_))) => {
                if let Err(error) = self.show_in_viewer(tab_id, path, window, cx) {
                    tracing::error!("{error:#}");
                }
            }
            _ => self.open_tab(TabContent::Viewer { path }, window, cx),
        }
    }

    /// Shows `path` in a viewer tab.
    fn show_in_viewer(
        &mut self,
        tab_id: TabId,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let Some(TabView::Viewer(viewer)) = self.views.get(&tab_id) else {
            return Err(eyre!("tab {tab_id:?} is not a viewer"));
        };
        let viewer = viewer.clone();
        self.set_tab_content(tab_id, TabContent::Viewer { path: path.clone() })?;
        viewer.update(cx, |viewer, cx| viewer.load(path, window, cx));
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
        match self.views.get(&self.active_tab()?.id())? {
            TabView::Viewer(viewer) => Some(viewer),
            TabView::Explorer(_) => None,
        }
    }

    fn active_explorer(&self) -> Option<&Entity<Explorer>> {
        match self.views.get(&self.active_tab()?.id())? {
            TabView::Explorer(explorer) => Some(explorer),
            TabView::Viewer(_) => None,
        }
    }

    /// The folder a new explorer starts in: the one of the active tab, or else
    /// the working directory.
    fn current_dir(&self) -> Option<PathBuf> {
        let from_tab = self.active_tab().and_then(|tab| match tab.content() {
            TabContent::Explorer { dir } => Some(dir.clone()),
            TabContent::Viewer { path } => path.parent().map(Path::to_path_buf),
        });
        from_tab.or_else(default_dir)
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
            .active_viewer()
            .and_then(|viewer| viewer.read(cx).neighbor(direction))
        else {
            return;
        };
        if let Err(error) = self.show_in_viewer(tab_id, path, window, cx) {
            tracing::error!("{error:#}");
        }
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(explorer) = self.active_explorer() {
            explorer.update(cx, Explorer::go_back);
        }
    }

    fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(explorer) = self.active_explorer() {
            explorer.update(cx, Explorer::go_forward);
        }
    }

    fn go_up(&mut self, _: &GoUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(explorer) = self.active_explorer() {
            explorer.update(cx, Explorer::go_up);
        }
    }

    #[expect(clippy::unused_self, reason = "`cx.listener` takes a method")]
    fn open(&mut self, _: &Open, window: &mut Window, cx: &mut Context<Self>) {
        Self::prompt_for_image(window, cx, Self::open_in_active_tab);
    }

    /// Opens an explorer of the current folder in a new tab.
    fn new_tab(&mut self, _: &NewTab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dir) = self.current_dir() {
            self.open_tab(TabContent::Explorer { dir }, window, cx);
        } else {
            tracing::error!("no folder to open: the working directory is unknown");
        }
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

    /// Closes the tab and releases its view, with a decoded image if any.
    fn close_tab(&mut self, pane_id: PaneId, tab_id: TabId, cx: &mut Context<Self>) {
        let result = self
            .workspace
            .pane_mut(pane_id)
            .and_then(|pane| pane.close_tab(tab_id));
        match result {
            Ok(()) => {
                self.views.remove(&tab_id);
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
        let content = match active.and_then(|tab_id| self.views.get(&tab_id)) {
            Some(view) => view.to_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x80_80_80))
                .child("Ctrl+N: open a folder, Ctrl+O: open an image")
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

/// The label of a tab: the file or folder name of what it shows.
fn tab_name(tab: &Tab) -> String {
    let path = match tab.content() {
        TabContent::Viewer { path } => path,
        TabContent::Explorer { dir } => dir,
    };
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
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
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::go_up))
            .child(self.render_layout(self.workspace.layout(), cx))
    }
}

/// The working directory, or else the home directory.
pub fn default_dir() -> Option<PathBuf> {
    std::env::current_dir().ok().or_else(std::env::home_dir)
}
