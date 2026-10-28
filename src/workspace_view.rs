use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use deltaflens_core::{Axis, Direction, Layout, PaneId, Side, Tab, TabContent, TabId, Workspace};
use eyre::{Result, eyre};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, Entity, FocusHandle, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PathPromptOptions, Pixels, Point, Window, actions, anchored,
    deferred, div, prelude::*, px, relative, rgb,
};

use crate::{
    explorer::{Explorer, ExplorerEvent},
    thumbnails::Thumbnails,
    viewer::Viewer,
    window_frame::{self, ROUNDING},
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
        SplitRight,
        SplitDown,
        FocusPaneLeft,
        FocusPaneRight,
        FocusPaneUp,
        FocusPaneDown,
    ]
);

pub const APP_NAME: &str = "deltaflens";

/// The height of a tab bar, which is also the title bar of the window.
const TAB_BAR_HEIGHT: Pixels = px(36.);
/// The width of the empty part of a tab bar kept for moving the window, even
/// when the tabs fill the bar.
const MIN_DRAG_WIDTH: Pixels = px(48.);

/// The color that marks the focused pane, on its active tab.
const FOCUS_COLOR: u32 = 0x5b_9b_e0;
/// The color of the lines between panes.
const DIVIDER_COLOR: u32 = 0x2a_2a_2a;

/// The menu opened by right-clicking a tab.
struct TabMenu {
    pane_id: PaneId,
    tab_id: TabId,
    position: Point<Pixels>,
}

/// What the tab bar of a pane at the top edge of the window does as part of
/// the title bar, when the app draws the window frame. Its empty part moves
/// the window.
#[derive(Clone, Copy)]
struct TitleBarPart {
    /// The window buttons are at the right end. True for the pane at the top
    /// right corner.
    controls: bool,
    round_top_left: bool,
    round_top_right: bool,
}

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
    /// The mouse was pressed on the empty part of a tab bar. The window starts
    /// moving when the mouse moves, so that a double click still reaches the
    /// tab bar.
    move_pending: bool,
    tab_menu: Option<TabMenu>,
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
            move_pending: false,
            tab_menu: None,
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
        if let Err(error) = self.workspace.open_tab(pane_id, content) {
            tracing::error!("{error:#}");
        }
        self.sync_views(window, cx);
    }

    /// Creates the views of tabs that have none, and releases the views of
    /// closed tabs, with their decoded images.
    fn sync_views(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut open = Vec::new();
        for pane_id in self.workspace.layout().pane_ids() {
            let Ok(pane) = self.workspace.pane(pane_id) else {
                continue;
            };
            for tab in pane.tabs() {
                open.push(tab.id());
                if !self.views.contains_key(&tab.id()) {
                    let view = self.create_view(tab.id(), tab.content().clone(), window, cx);
                    self.views.insert(tab.id(), view);
                }
            }
        }
        self.views.retain(|tab_id, _| open.contains(tab_id));
        cx.notify();
    }

    fn create_view(
        &self,
        tab_id: TabId,
        content: TabContent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> TabView {
        match content {
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
        }
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

    /// Closes the tab, and its pane if it was the last tab there, then releases
    /// its view with a decoded image if any.
    fn close_tab(&mut self, pane_id: PaneId, tab_id: TabId, cx: &mut Context<Self>) {
        match self.workspace.close_tab(pane_id, tab_id) {
            Ok(()) => {
                self.views.remove(&tab_id);
                cx.notify();
            }
            Err(error) => tracing::error!("{error:#}"),
        }
    }

    /// Splits the focused pane, copying its active tab into the new pane.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
        self.tab_menu = None;
        if let Err(error) = self
            .workspace
            .split_pane(self.workspace.focused_pane(), axis)
        {
            tracing::error!("{error:#}");
        }
        self.sync_views(window, cx);
    }

    fn split_right(&mut self, _: &SplitRight, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Horizontal, window, cx);
    }

    fn split_down(&mut self, _: &SplitDown, window: &mut Window, cx: &mut Context<Self>) {
        self.split(Axis::Vertical, window, cx);
    }

    fn focus_neighbor(&mut self, side: Side, cx: &mut Context<Self>) {
        if self.workspace.focus_neighbor(side) {
            cx.notify();
        }
    }

    fn focus_pane_left(&mut self, _: &FocusPaneLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_neighbor(Side::Left, cx);
    }

    fn focus_pane_right(&mut self, _: &FocusPaneRight, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_neighbor(Side::Right, cx);
    }

    fn focus_pane_up(&mut self, _: &FocusPaneUp, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_neighbor(Side::Up, cx);
    }

    fn focus_pane_down(&mut self, _: &FocusPaneDown, _: &mut Window, cx: &mut Context<Self>) {
        self.focus_neighbor(Side::Down, cx);
    }

    fn focus_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.workspace.focused_pane() != pane_id {
            if let Err(error) = self.workspace.focus_pane(pane_id) {
                tracing::error!("{error:#}");
            }
            cx.notify();
        }
    }

    fn render_tab_menu(menu: &TabMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .hover(|style| style.bg(rgb(0x3a_3a_3a)))
                .child(label)
        };
        let (pane_id, tab_id) = (menu.pane_id, menu.tab_id);
        // The split copies the tab the menu was opened on.
        let split_right = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.activate_tab(pane_id, tab_id, cx);
            this.split(Axis::Horizontal, window, cx);
        });
        let split_down = cx.listener(move |this, _: &ClickEvent, window, cx| {
            this.activate_tab(pane_id, tab_id, cx);
            this.split(Axis::Vertical, window, cx);
        });
        div()
            .flex()
            .flex_col()
            .min_w(px(160.))
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(rgb(0x3a_3a_3a))
            .bg(rgb(0x24_24_24))
            .text_sm()
            .text_color(rgb(0xd0_d0_d0))
            .shadow_md()
            // Keep clicks on the menu from reaching the tabs under it.
            .occlude()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.tab_menu = None;
                cx.notify();
            }))
            .child(item("split-right", "Split right").on_click(split_right))
            .child(item("split-down", "Split down").on_click(split_down))
            .child(item("close-tab", "Close tab").on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    this.tab_menu = None;
                    this.close_tab(pane_id, tab_id, cx);
                },
            )))
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

    /// The part each pane at the top of the window takes in the title bar, or
    /// nothing when the compositor draws the frame.
    fn title_bar_parts(&self, window: &Window) -> HashMap<PaneId, TitleBarPart> {
        let Some(tiling) = window_frame::client_tiling(window) else {
            return HashMap::new();
        };
        let layout = self.workspace.layout();
        let top = layout.top_pane_ids();
        let top_left = top.first().copied();
        let top_right = layout.top_right_pane_id();
        top.into_iter()
            .map(|pane_id| {
                let part = TitleBarPart {
                    controls: Some(pane_id) == top_right,
                    round_top_left: Some(pane_id) == top_left && !tiling.top && !tiling.left,
                    round_top_right: Some(pane_id) == top_right && !tiling.top && !tiling.right,
                };
                (pane_id, part)
            })
            .collect()
    }

    fn render_layout(
        &self,
        layout: &Layout,
        title_bar: &HashMap<PaneId, TitleBarPart>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            Layout::Pane(pane_id) => {
                let part = title_bar.get(pane_id).copied();
                self.render_pane(*pane_id, part, window, cx)
            }
            Layout::Split { axis, children } => div()
                .size_full()
                .flex()
                .when(*axis == Axis::Vertical, Styled::flex_col)
                .children(children.iter().enumerate().map(|(index, (child, ratio))| {
                    let cell = div()
                        .overflow_hidden()
                        .border_color(rgb(DIVIDER_COLOR))
                        .child(self.render_layout(child, title_bar, window, cx));
                    // A line on the leading side of every child but the first.
                    let has_divider = index > 0;
                    match axis {
                        Axis::Horizontal => cell
                            .h_full()
                            .w(relative(*ratio))
                            .when(has_divider, Styled::border_l_1),
                        Axis::Vertical => cell
                            .w_full()
                            .h(relative(*ratio))
                            .when(has_divider, Styled::border_t_1),
                    }
                }))
                .into_any_element(),
        }
    }

    fn render_pane(
        &self,
        pane_id: PaneId,
        title_bar: Option<TitleBarPart>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Ok(pane) = self.workspace.pane(pane_id) else {
            return div().into_any_element();
        };
        let active = pane.active_tab().map(Tab::id);
        // Only worth marking when there is another pane to tell it from.
        let marked = self.workspace.focused_pane() == pane_id
            && !matches!(self.workspace.layout(), Layout::Pane(_));
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
            // Pressing anywhere in a pane focuses it, so that keys act on it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.focus_pane(pane_id, cx)),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.focus_pane(pane_id, cx)),
            )
            // A pane at the top keeps its tab bar without tabs, to move the
            // window with.
            .when(!pane.tabs().is_empty() || title_bar.is_some(), |this| {
                this.child(Self::render_tab_bar(
                    pane_id,
                    pane.tabs(),
                    active.filter(|_| marked),
                    active,
                    title_bar,
                    window,
                    cx,
                ))
            })
            .child(div().flex_1().overflow_hidden().child(content))
            .into_any_element()
    }

    /// `marked` is the active tab of the focused pane, when the focus is worth
    /// showing.
    fn render_tab_bar(
        pane_id: PaneId,
        tabs: &[Tab],
        marked: Option<TabId>,
        active: Option<TabId>,
        title_bar: Option<TitleBarPart>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let round_top_left = title_bar.is_some_and(|part| part.round_top_left);
        let round_top_right = title_bar.is_some_and(|part| part.round_top_right);
        let tabs = div()
            .id(ElementId::Name(format!("tabs-{pane_id:?}").into()))
            .flex()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .children(tabs.iter().enumerate().map(|(index, tab)| {
                let tab_id = tab.id();
                let is_active = active == Some(tab_id);
                div()
                    .id(("tab", index))
                    .when(index == 0 && round_top_left, |this| {
                        this.rounded_tl(ROUNDING)
                    })
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
                    .relative()
                    .when(marked == Some(tab_id), |this| {
                        this.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_0()
                                .w_full()
                                .h(px(2.))
                                .bg(rgb(FOCUS_COLOR)),
                        )
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.activate_tab(pane_id, tab_id, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.tab_menu = Some(TabMenu {
                                pane_id,
                                tab_id,
                                position: event.position,
                            });
                            cx.notify();
                        }),
                    )
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
            }));
        div()
            .flex()
            .flex_shrink_0()
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .bg(rgb(0x14_14_14))
            .text_sm()
            .when(round_top_left, |this| this.rounded_tl(ROUNDING))
            .when(round_top_right, |this| this.rounded_tr(ROUNDING))
            .child(tabs)
            .when_some(title_bar, |this, part| {
                this.child(Self::render_drag_area(pane_id, window, cx))
                    .when(part.controls, |this| {
                        this.child(window_frame::window_controls(window, round_top_right))
                    })
            })
    }

    /// The empty part of a tab bar at the top of the window, which works as a
    /// title bar.
    fn render_drag_area(pane_id: PaneId, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let controls = window.window_controls();
        div()
            .id(ElementId::Name(format!("drag-area-{pane_id:?}").into()))
            .flex_1()
            .flex_shrink_0()
            .min_w(MIN_DRAG_WIDTH)
            .h_full()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, _, _| this.move_pending = true),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _, _| this.move_pending = false),
            )
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, _| {
                this.move_pending = false;
            }))
            .on_mouse_move(cx.listener(|this, _: &MouseMoveEvent, window, _| {
                if this.move_pending {
                    this.move_pending = false;
                    window.start_window_move();
                }
            }))
            .when(controls.maximize, |this| {
                this.on_click(|event, window, _| {
                    if event.click_count() == 2 {
                        window.zoom_window();
                    }
                })
            })
            .when(controls.window_menu, |this| {
                this.on_mouse_down(MouseButton::Right, |event, window, _| {
                    window.show_window_menu(event.position);
                })
            })
            .into_any_element()
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
        let title_bar = self.title_bar_parts(window);
        let content = div()
            .size_full()
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
            .on_action(cx.listener(Self::split_right))
            .on_action(cx.listener(Self::split_down))
            .on_action(cx.listener(Self::focus_pane_left))
            .on_action(cx.listener(Self::focus_pane_right))
            .on_action(cx.listener(Self::focus_pane_up))
            .on_action(cx.listener(Self::focus_pane_down))
            .child(self.render_layout(self.workspace.layout(), &title_bar, window, cx))
            .when_some(self.tab_menu.as_ref(), |this, menu| {
                this.child(
                    deferred(
                        anchored()
                            .position(menu.position)
                            .snap_to_window()
                            .child(Self::render_tab_menu(menu, cx)),
                    )
                    .with_priority(1),
                )
            });
        window_frame::window_frame(content, window)
    }
}

/// The working directory, or else the home directory.
pub fn default_dir() -> Option<PathBuf> {
    std::env::current_dir().ok().or_else(std::env::home_dir)
}
