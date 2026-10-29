use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use deltaflens_core::{
    Axis, Direction, DropZone, Layout, PaneId, Preset, Session, Side, Tab, TabContent, TabId,
    Workspace, drop_zone,
};
use eyre::{Result, eyre};
use gpui::{
    AnyElement, Bounds, ClickEvent, Context, CursorStyle, DragMoveEvent, ElementId, Entity,
    FocusHandle, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathPromptOptions,
    Pixels, Point, Rgba, SharedString, Subscription, Window, actions, div, prelude::*, px,
    relative,
};

use crate::{
    explorer::{DraggedImage, Explorer, ExplorerEvent},
    images::Images,
    menu::{ContextMenu, Menu},
    settings::SettingsStore,
    settings_view::SettingsView,
    theme::{Theme, theme},
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
        SwapPaneLeft,
        SwapPaneRight,
        SwapPaneUp,
        SwapPaneDown,
        LayoutSingle,
        LayoutTwoColumns,
        LayoutThreeColumns,
        LayoutGrid,
        LayoutExplorerAndViewer,
        ReopenClosedTab,
        OpenSettings,
    ]
);

pub const APP_NAME: &str = "deltaflens";

/// The height of a tab bar, which is also the title bar of the window.
const TAB_BAR_HEIGHT: Pixels = px(36.);
/// The width of the empty part of a tab bar kept for moving the window, even
/// when the tabs fill the bar.
const MIN_DRAG_WIDTH: Pixels = px(48.);

/// The width of the area around a line between panes that can be dragged.
const DIVIDER_HANDLE_WIDTH: Pixels = px(6.);
/// The smallest width and height of a pane when dragging a line between panes.
const MIN_PANE_SIZE: f32 = 120.;

/// A line between two children of a split, being dragged.
struct DraggedDivider {
    /// The indices of the children from the root to the split.
    path: Vec<usize>,
    /// The line is after this child.
    boundary: usize,
}

/// A tab being dragged.
struct DraggedTab {
    tab_id: TabId,
    label: SharedString,
}

/// What follows the mouse while dragging: the name of a tab, or nothing for a
/// line between panes.
struct DragPreview {
    label: Option<SharedString>,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div().when_some(self.label.clone(), |this, label| {
            this.px_3()
                .py_1()
                .rounded_sm()
                .bg(theme.element)
                .text_sm()
                .text_color(theme.text_strong)
                .child(label)
        })
    }
}

/// How a tab looks.
#[derive(Clone, Copy)]
struct TabLook {
    active: bool,
    /// The active tab of the focused pane, marked when there are other panes.
    marked: bool,
    /// The tab is at the top left corner of a window with rounded corners.
    round_top_left: bool,
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
    Settings(Entity<SettingsView>),
}

impl TabView {
    fn to_element(&self) -> AnyElement {
        match self {
            Self::Viewer(viewer) => viewer.clone().into_any_element(),
            Self::Explorer(explorer) => explorer.clone().into_any_element(),
            Self::Settings(settings) => settings.clone().into_any_element(),
        }
    }
}

/// The content of a window: renders the core [`Workspace`] and keeps the UI
/// state of each tab.
pub struct WorkspaceView {
    workspace: Workspace,
    views: HashMap<TabId, TabView>,
    thumbnails: Entity<Thumbnails>,
    images: Entity<Images>,
    settings: Entity<SettingsStore>,
    focus_handle: FocusHandle,
    /// The window title last set, to avoid setting it on every frame.
    title: String,
    /// The mouse was pressed on the empty part of a tab bar. The window starts
    /// moving when the mouse moves, so that a double click still reaches the
    /// tab bar.
    move_pending: bool,
    context_menu: Entity<ContextMenu>,
    /// Closes the menu when a key is pressed in the window.
    _close_menu_on_key: Subscription,
    /// Where a tab or image dragged over a pane would go.
    drop_target: Option<(PaneId, DropZone)>,
    /// The session last saved or tried to, to save it again only when it
    /// changes. `None` until the session is restored, so that the saved one
    /// is not replaced before.
    saved_session: Option<Session>,
}

impl WorkspaceView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        let settings = cx.new(SettingsStore::new);
        // Show errors in the settings as they come and go, and follow the
        // theme.
        cx.observe_in(&settings, window, |this, _, window, cx| {
            this.update_theme(window, cx);
            cx.notify();
        })
        .detach();
        cx.observe_window_appearance(window, Self::update_theme)
            .detach();
        // Keys would reach no action once the focused element is gone, such as
        // a text field in a tab that is no longer shown.
        cx.on_focus_lost(window, |this, window, cx| {
            window.focus(&this.focus_handle, cx);
        })
        .detach();
        let theme = Theme::for_choice(settings.read(cx).settings().theme, window.appearance());
        cx.set_global(theme);
        let context_menu = cx.new(|_| ContextMenu::default());
        let close_menu_on_key = close_menu_on_key(&context_menu, window, cx);
        Self {
            workspace: Workspace::new(),
            views: HashMap::new(),
            thumbnails: cx.new(Thumbnails::new),
            images: cx.new(|cx| Images::new(&settings, cx)),
            settings,
            focus_handle,
            title: APP_NAME.to_owned(),
            move_pending: false,
            context_menu,
            _close_menu_on_key: close_menu_on_key,
            drop_target: None,
            saved_session: None,
        }
    }

    /// Restores the layout and the tabs of the last session, leaving out
    /// files and folders that are gone. Returns whether any tab is restored.
    /// A session that cannot be read is left alone, so that it does not keep
    /// the application from starting.
    pub fn restore_session(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let restored = Session::path()
            .map(|path| Session::load(&path))
            .transpose()
            .map(Option::flatten)
            .and_then(|session| {
                session
                    .map(|session| session.restore(tab_exists))
                    .transpose()
            });
        let workspace = match restored {
            Ok(workspace) => workspace,
            Err(error) => {
                tracing::warn!("the last session is not restored: {error:#}");
                None
            }
        };
        let any_tab = workspace.as_ref().is_some_and(|workspace| {
            workspace
                .layout()
                .pane_ids()
                .into_iter()
                .any(|id| workspace.pane(id).is_ok_and(|pane| !pane.tabs().is_empty()))
        });
        if let Some(workspace) = workspace {
            self.workspace = workspace;
            self.views.clear();
            self.sync_views(window, cx);
        }
        self.saved_session = Some(Session::of(&self.workspace));
        any_tab
    }

    /// Saves the session if it changed since it was last saved. The file is
    /// small, so it is written at once, which keeps saves from overlapping.
    /// A save that fails is tried again on the next change.
    fn save_session(&mut self) {
        let Some(saved) = &self.saved_session else {
            return;
        };
        let session = Session::of(&self.workspace);
        if *saved == session {
            return;
        }
        let Some(path) = Session::path() else {
            return;
        };
        if let Err(error) = session.save(&path) {
            tracing::warn!("failed to save the session: {error:#}");
        }
        // Not tried again on every frame, which would repeat the warning.
        self.saved_session = Some(session);
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

    /// Tells each viewer whether its tab is on screen: the active tab of a
    /// pane. Hidden viewers let go of their images.
    fn update_visibility(&self, cx: &mut Context<Self>) {
        let visible: Vec<TabId> = self
            .workspace
            .layout()
            .pane_ids()
            .into_iter()
            .filter_map(|pane_id| self.workspace.pane(pane_id).ok()?.active_tab().map(Tab::id))
            .collect();
        for (tab_id, view) in &self.views {
            if let TabView::Viewer(viewer) = view {
                let visible = visible.contains(tab_id);
                viewer.update(cx, |viewer, cx| viewer.set_visible(visible, cx));
            }
        }
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
                let images = self.images.clone();
                let settings = &self.settings;
                TabView::Viewer(cx.new(|cx| Viewer::new(path, images, settings, cx)))
            }
            TabContent::Settings => {
                let settings = self.settings.clone();
                TabView::Settings(cx.new(|cx| SettingsView::new(settings, window, cx)))
            }
            TabContent::Explorer { dir } => {
                let thumbnails = self.thumbnails.clone();
                let settings = self.settings.clone();
                let context_menu = self.context_menu.clone();
                let explorer =
                    cx.new(|cx| Explorer::new(dir, thumbnails, settings, context_menu, cx));
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
                if let Err(error) = self.show_in_viewer(tab_id, path, Direction::Next, cx) {
                    tracing::error!("{error:#}");
                }
            }
            _ => self.open_tab(TabContent::Viewer { path }, window, cx),
        }
    }

    /// Shows `path` in a viewer tab, reached by moving in `direction`.
    fn show_in_viewer(
        &mut self,
        tab_id: TabId,
        path: PathBuf,
        direction: Direction,
        cx: &mut Context<Self>,
    ) -> Result<()> {
        let Some(TabView::Viewer(viewer)) = self.views.get(&tab_id) else {
            return Err(eyre!("tab {tab_id:?} is not a viewer"));
        };
        let viewer = viewer.clone();
        self.set_tab_content(tab_id, TabContent::Viewer { path: path.clone() })?;
        viewer.update(cx, |viewer, cx| viewer.load(path, direction, cx));
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
            TabView::Explorer(_) | TabView::Settings(_) => None,
        }
    }

    fn active_explorer(&self) -> Option<&Entity<Explorer>> {
        match self.views.get(&self.active_tab()?.id())? {
            TabView::Explorer(explorer) => Some(explorer),
            TabView::Viewer(_) | TabView::Settings(_) => None,
        }
    }

    /// The folder a new explorer starts in: the one of the active tab, or else
    /// the working directory.
    fn current_dir(&self) -> Option<PathBuf> {
        let from_tab = self.active_tab().and_then(|tab| match tab.content() {
            TabContent::Explorer { dir } => Some(dir.clone()),
            TabContent::Viewer { path } => path.parent().map(Path::to_path_buf),
            TabContent::Settings => None,
        });
        from_tab.or_else(default_dir)
    }

    fn reset_view(&mut self, _: &ResetView, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(viewer) = self.active_viewer() {
            viewer.update(cx, Viewer::reset_view);
        }
    }

    fn previous_image(&mut self, _: &PreviousImage, _: &mut Window, cx: &mut Context<Self>) {
        self.move_in_folder(Direction::Previous, cx);
    }

    fn next_image(&mut self, _: &NextImage, _: &mut Window, cx: &mut Context<Self>) {
        self.move_in_folder(Direction::Next, cx);
    }

    /// Shows the previous or next image of the folder in the active tab.
    fn move_in_folder(&mut self, direction: Direction, cx: &mut Context<Self>) {
        let Some(tab_id) = self.active_tab().map(Tab::id) else {
            return;
        };
        let Some(path) = self
            .active_viewer()
            .and_then(|viewer| viewer.read(cx).neighbor(direction))
        else {
            return;
        };
        if let Err(error) = self.show_in_viewer(tab_id, path, direction, cx) {
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

    /// Uses the theme of the settings, which may follow the appearance of
    /// the system.
    fn update_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let choice = self.settings.read(cx).settings().theme;
        let new = Theme::for_choice(choice, window.appearance());
        if cx.try_global::<Theme>() != Some(&new) {
            cx.set_global(new);
            cx.refresh_windows();
        }
    }

    /// Shows the settings tab, opening it in the focused pane if there is
    /// none.
    fn open_settings(&mut self, _: &OpenSettings, window: &mut Window, cx: &mut Context<Self>) {
        match self.workspace.find_tab(&TabContent::Settings) {
            Some(tab_id) => {
                if let Err(error) = self.workspace.activate_tab(tab_id) {
                    tracing::error!("{error:#}");
                }
                cx.notify();
            }
            None => self.open_tab(TabContent::Settings, window, cx),
        }
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

    /// Moves a dragged tab to `index` in `pane_id`, from any pane.
    fn move_tab(
        &mut self,
        dragged: &DraggedTab,
        pane_id: PaneId,
        index: usize,
        cx: &mut Context<Self>,
    ) {
        if let Err(error) = self
            .workspace
            .move_tab(dragged.tab_id, pane_id, Some(index))
        {
            tracing::error!("{error:#}");
        }
        cx.notify();
    }

    /// Opens a dragged image in a new tab at `index` in `pane_id`.
    fn open_image_at(
        &mut self,
        dragged: &DraggedImage,
        pane_id: PaneId,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = TabContent::Viewer {
            path: dragged.path.clone(),
        };
        let result = self
            .workspace
            .open_tab(pane_id, content)
            .and_then(|tab_id| {
                self.workspace.focus_pane(pane_id)?;
                self.workspace.move_tab(tab_id, pane_id, Some(index))
            });
        if let Err(error) = result {
            tracing::error!("{error:#}");
        }
        self.sync_views(window, cx);
    }

    /// Remembers where a tab or image dragged over `pane_id` would go, from the
    /// mouse at `position` and the pane's content at `bounds`.
    fn update_drop_target(
        &mut self,
        pane_id: PaneId,
        bounds: Bounds<Pixels>,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let target = if bounds.contains(&position) {
            let offset = position - bounds.origin;
            let zone = drop_zone(
                f32::from(bounds.size.width),
                f32::from(bounds.size.height),
                f32::from(offset.x),
                f32::from(offset.y),
            );
            Some((pane_id, zone))
        } else if self.drop_target.is_some_and(|(id, _)| id == pane_id) {
            None
        } else {
            return;
        };
        if self.drop_target != target {
            self.drop_target = target;
            cx.notify();
        }
    }

    /// Where a drop on `pane_id` goes, as last shown.
    fn take_drop_zone(&mut self, pane_id: PaneId) -> DropZone {
        match self.drop_target.take() {
            Some((id, zone)) if id == pane_id => zone,
            _ => DropZone::Center,
        }
    }

    /// Moves a tab dropped on the content of `pane_id` into it, or into a new
    /// pane split off on the edge it was dropped on.
    fn drop_tab_on_pane(&mut self, dragged: &DraggedTab, pane_id: PaneId, cx: &mut Context<Self>) {
        let result = match self.take_drop_zone(pane_id) {
            DropZone::Center => self.workspace.move_tab(dragged.tab_id, pane_id, None),
            DropZone::Edge(side) => self
                .workspace
                .move_tab_to_split(dragged.tab_id, pane_id, side)
                .map(|_| ()),
        };
        if let Err(error) = result {
            tracing::error!("{error:#}");
        }
        cx.notify();
    }

    /// Opens an image dropped on the content of `pane_id` in a new tab there,
    /// or in a new pane split off on the edge it was dropped on.
    fn drop_image_on_pane(
        &mut self,
        dragged: &DraggedImage,
        pane_id: PaneId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let content = TabContent::Viewer {
            path: dragged.path.clone(),
        };
        let result = match self.take_drop_zone(pane_id) {
            DropZone::Center => self
                .workspace
                .focus_pane(pane_id)
                .and_then(|()| self.workspace.open_tab(pane_id, content)),
            DropZone::Edge(side) => self.workspace.open_tab_in_split(pane_id, side, content),
        };
        if let Err(error) = result {
            tracing::error!("{error:#}");
        }
        self.sync_views(window, cx);
    }

    /// Splits the focused pane, copying its active tab into the new pane.
    fn split(&mut self, axis: Axis, window: &mut Window, cx: &mut Context<Self>) {
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

    fn swap_with_neighbor(&mut self, side: Side, cx: &mut Context<Self>) {
        if self.workspace.swap_with_neighbor(side) {
            cx.notify();
        }
    }

    fn swap_pane_left(&mut self, _: &SwapPaneLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.swap_with_neighbor(Side::Left, cx);
    }

    fn swap_pane_right(&mut self, _: &SwapPaneRight, _: &mut Window, cx: &mut Context<Self>) {
        self.swap_with_neighbor(Side::Right, cx);
    }

    fn swap_pane_up(&mut self, _: &SwapPaneUp, _: &mut Window, cx: &mut Context<Self>) {
        self.swap_with_neighbor(Side::Up, cx);
    }

    fn swap_pane_down(&mut self, _: &SwapPaneDown, _: &mut Window, cx: &mut Context<Self>) {
        self.swap_with_neighbor(Side::Down, cx);
    }

    fn apply_preset(&mut self, preset: Preset, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace.apply_preset(preset);
        self.sync_views(window, cx);
    }

    fn layout_single(&mut self, _: &LayoutSingle, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_preset(Preset::Single, window, cx);
    }

    fn layout_two_columns(
        &mut self,
        _: &LayoutTwoColumns,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_preset(Preset::TwoColumns, window, cx);
    }

    fn layout_three_columns(
        &mut self,
        _: &LayoutThreeColumns,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_preset(Preset::ThreeColumns, window, cx);
    }

    fn layout_grid(&mut self, _: &LayoutGrid, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_preset(Preset::Grid, window, cx);
    }

    fn layout_explorer_and_viewer(
        &mut self,
        _: &LayoutExplorerAndViewer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_preset(Preset::ExplorerAndViewer, window, cx);
    }

    fn focus_pane(&mut self, pane_id: PaneId, cx: &mut Context<Self>) {
        if self.workspace.focused_pane() != pane_id {
            if let Err(error) = self.workspace.focus_pane(pane_id) {
                tracing::error!("{error:#}");
            }
            cx.notify();
        }
    }

    /// Opens the menu of a tab at `position`.
    fn show_tab_menu(
        &mut self,
        pane_id: PaneId,
        tab_id: TabId,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let this = cx.entity().downgrade();
        // The split copies the tab the menu was opened on.
        let split = {
            let this = this.clone();
            move |axis| {
                let this = this.clone();
                move |window: &mut Window, cx: &mut gpui::App| {
                    let _ = this.update(cx, |this, cx| {
                        this.activate_tab(pane_id, tab_id, cx);
                        this.split(axis, window, cx);
                    });
                }
            }
        };
        let close = move |_: &mut Window, cx: &mut gpui::App| {
            let _ = this.update(cx, |this, cx| this.close_tab(pane_id, tab_id, cx));
        };
        let menu = Menu::new(position)
            .item("Split right", split(Axis::Horizontal))
            .item("Split down", split(Axis::Vertical))
            .item("Close tab", close);
        self.context_menu
            .update(cx, |context_menu, cx| context_menu.show(menu, cx));
    }

    /// Reopens the most recently closed tab, skipping those whose file or
    /// folder is gone.
    fn reopen_closed_tab(
        &mut self,
        _: &ReopenClosedTab,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.workspace.reopen_closed_tab(tab_exists).is_some() {
            self.sync_views(window, cx);
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

    /// `path` is the indices of the children from the root to `layout`.
    fn render_layout(
        &self,
        layout: &Layout,
        path: &[usize],
        title_bar: &HashMap<PaneId, TitleBarPart>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match layout {
            Layout::Pane(pane_id) => {
                let part = title_bar.get(pane_id).copied();
                self.render_pane(*pane_id, part, window, cx)
            }
            Layout::Split { axis, children } => {
                let axis = *axis;
                // A line after every child but the last.
                let mut offset = 0.;
                let mut dividers = Vec::new();
                for (boundary, (_, ratio)) in children
                    .iter()
                    .enumerate()
                    .take(children.len().saturating_sub(1))
                {
                    offset += ratio;
                    dividers.push(Self::render_divider(path, boundary, axis, offset, cx));
                }
                let split_path = path.to_vec();
                div()
                    .id(ElementId::Name(format!("split-{path:?}").into()))
                    .size_full()
                    .relative()
                    .flex()
                    .when(axis == Axis::Vertical, Styled::flex_col)
                    .on_drag_move(cx.listener(
                        move |this, event: &DragMoveEvent<DraggedDivider>, _, cx| {
                            let Some(divider) =
                                event.dragged_item().downcast_ref::<DraggedDivider>()
                            else {
                                return;
                            };
                            if divider.path != split_path {
                                return;
                            }
                            let position = event.event.position - event.bounds.origin;
                            let (offset, extent) = match axis {
                                Axis::Horizontal => (position.x, event.bounds.size.width),
                                Axis::Vertical => (position.y, event.bounds.size.height),
                            };
                            let result = this.workspace.move_boundary(
                                &split_path,
                                divider.boundary,
                                f32::from(offset),
                                f32::from(extent),
                                MIN_PANE_SIZE,
                            );
                            if let Err(error) = result {
                                tracing::error!("{error:#}");
                            }
                            cx.notify();
                        },
                    ))
                    .children(children.iter().enumerate().map(|(index, (child, ratio))| {
                        let mut child_path = path.to_vec();
                        child_path.push(index);
                        let cell = div()
                            .overflow_hidden()
                            .border_color(theme(cx).border)
                            .child(self.render_layout(child, &child_path, title_bar, window, cx));
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
                    .children(dividers)
                    .into_any_element()
            }
        }
    }

    /// The area around the line after child `boundary` of the split at `path`,
    /// at `offset` (a ratio of the split) from its start. Dragging it resizes
    /// the two children next to it, and double-clicking it gives every child
    /// the same size.
    fn render_divider(
        path: &[usize],
        boundary: usize,
        axis: Axis,
        offset: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let dragged = DraggedDivider {
            path: path.to_vec(),
            boundary,
        };
        let equalize_path = path.to_vec();
        let accent = theme(cx).accent;
        let handle = div()
            .id(ElementId::Name(
                format!("divider-{path:?}-{boundary}").into(),
            ))
            .absolute()
            // Keep presses on the line from also panning a viewer or moving the
            // window under it.
            .occlude()
            .hover(move |style| style.bg(accent))
            .on_drag(dragged, |_, _, _, cx| {
                cx.new(|_| DragPreview { label: None })
            })
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    if let Err(error) = this.workspace.equalize(&equalize_path) {
                        tracing::error!("{error:#}");
                    }
                    cx.notify();
                }
            }));
        // Centered on the line.
        let half = -DIVIDER_HANDLE_WIDTH / 2.;
        match axis {
            Axis::Horizontal => handle
                .top_0()
                .h_full()
                .left(relative(offset))
                .ml(half)
                .w(DIVIDER_HANDLE_WIDTH)
                .cursor(CursorStyle::ResizeLeftRight),
            Axis::Vertical => handle
                .left_0()
                .w_full()
                .top(relative(offset))
                .mt(half)
                .h(DIVIDER_HANDLE_WIDTH)
                .cursor(CursorStyle::ResizeUpDown),
        }
        .into_any_element()
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
                .text_color(theme(cx).text_placeholder)
                .child("Ctrl+N: open a folder, Ctrl+O: open an image")
                .into_any_element(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            // Pressing anywhere in a pane focuses it, so that keys act on it.
            // This comes before the elements in the pane, which may stop the
            // event, such as a text field.
            .capture_any_mouse_down(
                cx.listener(move |this, _: &MouseDownEvent, _, cx| this.focus_pane(pane_id, cx)),
            )
            // Take the keys back from a text field, unless it was pressed.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }),
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
            .child(self.render_pane_content(pane_id, content, cx))
            .into_any_element()
    }

    /// The content of a pane, which takes tabs and images dropped on it.
    fn render_pane_content(
        &self,
        pane_id: PaneId,
        content: AnyElement,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let zone = self
            .drop_target
            .filter(|(id, _)| *id == pane_id && cx.has_active_drag())
            .map(|(_, zone)| zone);
        div()
            .flex_1()
            .overflow_hidden()
            .relative()
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DraggedTab>, _, cx| {
                    this.update_drop_target(pane_id, event.bounds, event.event.position, cx);
                }),
            )
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<DraggedImage>, _, cx| {
                    this.update_drop_target(pane_id, event.bounds, event.event.position, cx);
                }),
            )
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.drop_tab_on_pane(dragged, pane_id, cx);
            }))
            .on_drop(
                cx.listener(move |this, dragged: &DraggedImage, window, cx| {
                    this.drop_image_on_pane(dragged, pane_id, window, cx);
                }),
            )
            .child(content)
            .when_some(zone, |this, zone| {
                this.child(render_drop_zone(zone, theme(cx).drop_zone))
            })
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
        let tab_count = tabs.len();
        let round_top_left = title_bar.is_some_and(|part| part.round_top_left);
        let round_top_right = title_bar.is_some_and(|part| part.round_top_right);
        let tabs = div()
            .id(ElementId::Name(format!("tabs-{pane_id:?}").into()))
            .flex()
            .min_w_0()
            .h_full()
            .overflow_x_scroll()
            .children(tabs.iter().enumerate().map(|(index, tab)| {
                let look = TabLook {
                    active: active == Some(tab.id()),
                    marked: marked == Some(tab.id()),
                    round_top_left: index == 0 && round_top_left,
                };
                Self::render_tab(pane_id, index, tab, look, cx)
            }));
        div()
            .flex()
            .flex_shrink_0()
            .w_full()
            .h(TAB_BAR_HEIGHT)
            .bg(theme(cx).tab_bar)
            .text_sm()
            .when(round_top_left, |this| this.rounded_tl(ROUNDING))
            .when(round_top_right, |this| this.rounded_tr(ROUNDING))
            .child(tabs)
            .child(Self::render_rest_of_tab_bar(
                pane_id,
                tab_count,
                title_bar.is_some(),
                window,
                cx,
            ))
            .when(title_bar.is_some_and(|part| part.controls), |this| {
                this.child(window_frame::window_controls(window, round_top_right, cx))
            })
    }

    fn render_tab(
        pane_id: PaneId,
        index: usize,
        tab: &Tab,
        look: TabLook,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tab_id = tab.id();
        let label = SharedString::from(tab_name(tab));
        let theme = theme(cx);
        div()
            .id(("tab", index))
            .when(look.round_top_left, |this| this.rounded_tl(ROUNDING))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .max_w(px(240.))
            .h_full()
            .pl_3()
            .pr_1()
            .border_r_1()
            .border_color(theme.border)
            .bg(if look.active {
                theme.background
            } else {
                theme.tab_bar
            })
            .text_color(if look.active {
                theme.text_strong
            } else {
                theme.text_muted
            })
            .relative()
            .when(look.marked, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .w_full()
                        .h(px(2.))
                        .bg(theme.accent),
                )
            })
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.activate_tab(pane_id, tab_id, cx);
            }))
            // Dragging a tab moves it; dropping it (or an image) on a tab puts
            // it where that tab is, in any pane.
            .on_drag(
                DraggedTab {
                    tab_id,
                    label: label.clone(),
                },
                |dragged, _, _, cx| {
                    cx.new(|_| DragPreview {
                        label: Some(dragged.label.clone()),
                    })
                },
            )
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(theme.drop_target))
            .drag_over::<DraggedImage>(move |style, _, _, _| style.bg(theme.drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab(dragged, pane_id, index, cx);
            }))
            .on_drop(
                cx.listener(move |this, dragged: &DraggedImage, window, cx| {
                    this.open_image_at(dragged, pane_id, index, window, cx);
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.show_tab_menu(pane_id, tab_id, event.position, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    this.close_tab(pane_id, tab_id, cx);
                }),
            )
            .child(div().min_w_0().truncate().child(label))
            .child(
                div()
                    .id("close")
                    .flex_shrink_0()
                    .px_1()
                    .rounded_sm()
                    .hover(move |style| style.bg(theme.element_hover))
                    .child("×")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.close_tab(pane_id, tab_id, cx);
                    })),
            )
            .into_any_element()
    }

    /// The part of a tab bar right of the tabs. A tab dropped on it moves to
    /// the end. At the top of the window, it works as a title bar when
    /// `moves_window` is set.
    fn render_rest_of_tab_bar(
        pane_id: PaneId,
        tab_count: usize,
        moves_window: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let controls = window.window_controls();
        let theme = theme(cx);
        let rest = div()
            .id(ElementId::Name(format!("tab-bar-rest-{pane_id:?}").into()))
            .flex_1()
            .h_full()
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(theme.drop_target))
            .drag_over::<DraggedImage>(move |style, _, _, _| style.bg(theme.drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab(dragged, pane_id, tab_count, cx);
            }))
            .on_drop(
                cx.listener(move |this, dragged: &DraggedImage, window, cx| {
                    this.open_image_at(dragged, pane_id, tab_count, window, cx);
                }),
            );
        if !moves_window {
            return rest.into_any_element();
        }
        rest.flex_shrink_0()
            .min_w(MIN_DRAG_WIDTH)
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

/// The part of a pane that a drop in `zone` would take: all of it, or the half
/// on that side.
/// The part of a pane that a dragged tab or image would take, shown over it
/// in `color`.
fn render_drop_zone(zone: DropZone, color: Rgba) -> impl IntoElement {
    let overlay = div().absolute().bg(color);
    match zone {
        DropZone::Center => overlay.inset_0(),
        DropZone::Edge(Side::Left) => overlay.top_0().left_0().h_full().w_1_2(),
        DropZone::Edge(Side::Right) => overlay.top_0().right_0().h_full().w_1_2(),
        DropZone::Edge(Side::Up) => overlay.top_0().left_0().w_full().h_1_2(),
        DropZone::Edge(Side::Down) => overlay.bottom_0().left_0().w_full().h_1_2(),
    }
}

/// Whether the file or folder a tab shows is still there.
fn tab_exists(content: &TabContent) -> bool {
    match content {
        TabContent::Viewer { path } => path.is_file(),
        TabContent::Explorer { dir } => dir.is_dir(),
        TabContent::Settings => true,
    }
}

/// The label of a tab: the file or folder name of what it shows.
fn tab_name(tab: &Tab) -> String {
    let path = match tab.content() {
        TabContent::Viewer { path } => path,
        TabContent::Explorer { dir } => dir,
        TabContent::Settings => return "Settings".to_owned(),
    };
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
        .into_owned()
}

impl Render for WorkspaceView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.update_title(window);
        self.update_visibility(cx);
        self.save_session();
        // A drag that ended outside every pane leaves nothing to show.
        if !cx.has_active_drag() {
            self.drop_target = None;
        }
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
            .on_action(cx.listener(Self::swap_pane_left))
            .on_action(cx.listener(Self::swap_pane_right))
            .on_action(cx.listener(Self::swap_pane_up))
            .on_action(cx.listener(Self::swap_pane_down))
            .on_action(cx.listener(Self::layout_single))
            .on_action(cx.listener(Self::layout_two_columns))
            .on_action(cx.listener(Self::layout_three_columns))
            .on_action(cx.listener(Self::layout_grid))
            .on_action(cx.listener(Self::layout_explorer_and_viewer))
            .on_action(cx.listener(Self::reopen_closed_tab))
            .on_action(cx.listener(Self::open_settings))
            .flex()
            .flex_col()
            .child(div().flex_1().min_h_0().child(self.render_layout(
                self.workspace.layout(),
                &[],
                &title_bar,
                window,
                cx,
            )))
            .children(self.render_settings_errors(cx))
            .child(self.context_menu.clone());
        window_frame::window_frame(content, window, cx)
    }
}

impl WorkspaceView {
    /// A bar at the bottom that tells about mistakes in the settings file,
    /// until they are fixed.
    fn render_settings_errors(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let errors = self.settings.read(cx).errors();
        let first = errors.first()?;
        let more = match errors.len() {
            1 => String::new(),
            count => format!(" ({} more)", count - 1),
        };
        Some(
            div()
                .flex_shrink_0()
                .px_3()
                .py_1()
                .text_sm()
                .bg(theme(cx).error_background)
                .text_color(theme(cx).error_text)
                .child(format!(
                    "Settings: {first}{more} Ctrl+, shows the settings."
                )),
        )
    }
}

/// Closes the menu of `window` when a key other than a modifier is pressed
/// there, before the key reaches an action. Otherwise the key could move to
/// another image or folder, and leave a menu that acts on the one before.
/// Escape only closes the menu; other keys do what they do as well.
fn close_menu_on_key(
    context_menu: &Entity<ContextMenu>,
    window: &Window,
    cx: &mut gpui::App,
) -> Subscription {
    let context_menu = context_menu.downgrade();
    let window_handle = window.window_handle();
    cx.intercept_keystrokes(move |event, window, cx| {
        let key = event.keystroke.key.as_str();
        // A modifier pressed and released alone comes as a keystroke too.
        let modifier_only = matches!(key, "shift" | "control" | "alt" | "platform" | "function");
        if window.window_handle() != window_handle || modifier_only {
            return;
        }
        let closed = context_menu
            .update(cx, ContextMenu::dismiss)
            .unwrap_or_default();
        if closed && key == "escape" {
            cx.stop_propagation();
        }
    })
}

/// The working directory, or else the home directory.
pub fn default_dir() -> Option<PathBuf> {
    std::env::current_dir().ok().or_else(std::env::home_dir)
}
