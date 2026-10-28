use std::{
    ops::Range,
    path::{Path, PathBuf},
};

use deltaflens_core::{Entry, EntryKind, History, Sort, list_entries, sort_entries};
use gpui::{
    AnyElement, ClickEvent, Context, Entity, EventEmitter, MouseButton, MouseDownEvent,
    NavigationDirection, ObjectFit, Pixels, ScrollStrategy, SharedString, Task,
    UniformListScrollHandle, Window, container_query, div, img, prelude::*, px, rgb, uniform_list,
};

use crate::thumbnails::{ThumbnailKey, Thumbnails};

/// The size of a grid cell, including its padding.
const CELL_WIDTH: f32 = 152.;
const CELL_HEIGHT: f32 = 168.;
/// The size of the square that holds the icon of an entry.
const ICON_SIZE: f32 = 128.;
/// The space between the grid and the edges of the explorer.
const GRID_PADDING: f32 = 8.;

pub enum ExplorerEvent {
    /// The explorer moved to another folder.
    Navigated(PathBuf),
    /// An image was double-clicked.
    OpenImage(PathBuf),
}

/// Shows the folders and images of a folder as a grid, like a file manager.
pub struct Explorer {
    history: History,
    sort: Sort,
    /// The entries of the current folder, sorted by `sort`. Empty while the
    /// folder is loading.
    entries: Vec<Entry>,
    error: Option<SharedString>,
    /// Dropping it cancels the load, so that only the latest folder is shown.
    load_task: Option<Task<()>>,
    scroll_handle: UniformListScrollHandle,
    thumbnails: Entity<Thumbnails>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    pub fn new(dir: PathBuf, thumbnails: Entity<Thumbnails>, cx: &mut Context<Self>) -> Self {
        // Redraw when a thumbnail is ready.
        cx.observe(&thumbnails, |_, _, cx| cx.notify()).detach();
        let mut this = Self {
            history: History::new(dir),
            sort: Sort::default(),
            entries: Vec::new(),
            error: None,
            load_task: None,
            scroll_handle: UniformListScrollHandle::new(),
            thumbnails,
        };
        this.load(cx);
        this
    }

    pub fn dir(&self) -> &Path {
        self.history.current()
    }

    pub fn navigate(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        if dir != self.dir() {
            self.history.navigate(dir);
            self.on_navigated(cx);
        }
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.history.go_back() {
            self.on_navigated(cx);
        }
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.history.go_forward() {
            self.on_navigated(cx);
        }
    }

    pub fn go_up(&mut self, cx: &mut Context<Self>) {
        if self.history.go_up() {
            self.on_navigated(cx);
        }
    }

    fn on_navigated(&mut self, cx: &mut Context<Self>) {
        cx.emit(ExplorerEvent::Navigated(self.dir().to_owned()));
        let dir = self.dir().to_owned();
        self.thumbnails
            .update(cx, |thumbnails, _| thumbnails.keep_requests_in(&dir));
        self.scroll_handle.scroll_to_item(0, ScrollStrategy::Top);
        self.load(cx);
    }

    /// Lists and sorts the current folder in the background.
    fn load(&mut self, cx: &mut Context<Self>) {
        self.entries.clear();
        self.error = None;
        let dir = self.dir().to_owned();
        let sort = self.sort;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut entries = list_entries(&dir)?;
                    sort_entries(&mut entries, sort);
                    eyre::Ok(entries)
                })
                .await;
            // Fails only if the explorer is gone.
            let _ = this.update(cx, |this, cx| {
                this.load_task = None;
                match result {
                    Ok(entries) => this.entries = entries,
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn open_entry(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(index) else {
            return;
        };
        match entry.kind {
            EntryKind::Folder => self.navigate(entry.path.clone(), cx),
            EntryKind::Image => cx.emit(ExplorerEvent::OpenImage(entry.path.clone())),
        }
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .bg(rgb(0x18_18_18))
            .border_b_1()
            .border_color(rgb(0x2a_2a_2a))
            .child(nav_button(
                "back",
                "←",
                self.history.can_go_back(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx)),
            ))
            .child(nav_button(
                "forward",
                "→",
                self.history.can_go_forward(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_forward(cx)),
            ))
            .child(nav_button(
                "up",
                "↑",
                self.dir().parent().is_some(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_up(cx)),
            ))
            .child(self.render_path_bar(cx))
    }

    /// Each folder from the root to the current one, as a button that moves
    /// there.
    fn render_path_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ancestors: Vec<&Path> = self.dir().ancestors().collect();
        ancestors.reverse();
        let last = ancestors.len().saturating_sub(1);
        div()
            .id("path-bar")
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .ml_2()
            .overflow_x_scroll()
            .text_sm()
            .children(ancestors.into_iter().enumerate().map(|(index, dir)| {
                let name = dir.file_name().map_or_else(
                    || dir.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                );
                let target = dir.to_path_buf();
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .when(index > 0, |this| {
                        this.child(div().px_0p5().text_color(rgb(0x60_60_60)).child("›"))
                    })
                    .child(
                        div()
                            .id(("path", index))
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .text_color(rgb(if index == last {
                                0xe0_e0_e0
                            } else {
                                0xa0_a0_a0
                            }))
                            .hover(|style| style.bg(rgb(0x2a_2a_2a)))
                            .child(name)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.navigate(target.clone(), cx);
                            })),
                    )
            }))
    }

    fn render_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        let message = if let Some(error) = &self.error {
            Some(error.clone())
        } else if self.load_task.is_some() {
            None
        } else if self.entries.is_empty() {
            Some("No folders or images".into())
        } else {
            return self.render_entries(cx);
        };
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .p_4()
            .text_color(rgb(0x80_80_80))
            .children(message)
            .into_any_element()
    }

    /// Lays the entries out in rows that fill the width, and renders only the
    /// visible rows.
    fn render_entries(&self, cx: &mut Context<Self>) -> AnyElement {
        let explorer = cx.entity();
        let count = self.entries.len();
        let scroll_handle = self.scroll_handle.clone();
        container_query(move |size, _, _| {
            let columns = column_count(size.width);
            uniform_list(
                "entries",
                count.div_ceil(columns),
                move |rows, window, cx| {
                    explorer.update(cx, |this, cx| this.render_rows(rows, columns, window, cx))
                },
            )
            .track_scroll(&scroll_handle)
            .size_full()
            .py(px(GRID_PADDING))
        })
        .into_any_element()
    }

    fn render_rows(
        &self,
        rows: Range<usize>,
        columns: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<impl IntoElement + use<>> {
        // Truncation is intended; the value is small and positive.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let thumbnail_size = (ICON_SIZE * window.scale_factor()).round() as u32;
        rows.map(|row| {
            let start = row * columns;
            let end = (start + columns).min(self.entries.len());
            div()
                .flex()
                .px(px(GRID_PADDING))
                .children((start..end).map(|index| self.render_cell(index, thumbnail_size, cx)))
        })
        .collect()
    }

    fn render_cell(
        &self,
        index: usize,
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let entry = &self.entries[index];
        let name = entry
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let icon = match entry.kind {
            EntryKind::Folder => folder_icon().into_any_element(),
            EntryKind::Image => {
                let key = ThumbnailKey {
                    path: entry.path.clone(),
                    modified: entry.modified,
                    size: thumbnail_size,
                };
                match self
                    .thumbnails
                    .update(cx, |thumbnails, cx| thumbnails.get(&key, cx))
                {
                    Some(image) => img(image)
                        .size_full()
                        .object_fit(ObjectFit::Contain)
                        .into_any_element(),
                    None => image_placeholder(&entry.path).into_any_element(),
                }
            }
        };
        div()
            .id(("entry", index))
            .flex()
            .flex_col()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .w(px(CELL_WIDTH))
            .h(px(CELL_HEIGHT))
            .p_1()
            .rounded_md()
            .hover(|style| style.bg(rgb(0x2a_2a_2a)))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(ICON_SIZE))
                    .child(icon),
            )
            .child(
                div()
                    .w_full()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis_middle()
                    .text_center()
                    .text_sm()
                    .text_color(rgb(0xd0_d0_d0))
                    .child(name),
            )
            // A double click opens the entry, as in Dolphin. A single click
            // does nothing until entries can be selected.
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.open_entry(index, cx);
                }
            }))
    }
}

impl Render for Explorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.go_back(cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|this, _: &MouseDownEvent, _, cx| this.go_forward(cx)),
            )
            .child(self.render_toolbar(cx))
            .child(div().flex_1().min_h_0().child(self.render_grid(cx)))
    }
}

/// How many cells fit in a row of `width`, at least one.
fn column_count(width: Pixels) -> usize {
    let available = f32::from(width) - 2. * GRID_PADDING;
    // Truncation is intended; the value is small and non-negative.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let columns = (available / CELL_WIDTH).floor().max(1.) as usize;
    columns
}

fn nav_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded_sm()
        .text_color(rgb(if enabled { 0xd0_d0_d0 } else { 0x50_50_50 }))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(0x2a_2a_2a)))
                .on_click(on_click)
        })
        .child(label)
}

/// A folder drawn with plain shapes, so that no icon font is needed.
fn folder_icon() -> impl IntoElement {
    let color = rgb(0xc8_a0_4a);
    div()
        .flex()
        .flex_col()
        .w(px(104.))
        .child(div().w(px(40.)).h(px(10.)).rounded_t_md().bg(color))
        .child(
            div()
                .w_full()
                .h(px(74.))
                .rounded_md()
                .rounded_tl_none()
                .bg(color),
        )
}

/// Stands in for the thumbnail until it is ready, or if the image cannot be
/// decoded: the extension of the image in a frame.
fn image_placeholder(path: &Path) -> impl IntoElement {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_uppercase();
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(96.))
        .rounded_md()
        .border_1()
        .border_color(rgb(0x50_50_50))
        .bg(rgb(0x26_26_26))
        .text_xs()
        .text_color(rgb(0x90_90_90))
        .child(extension)
}
