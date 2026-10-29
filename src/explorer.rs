use std::{
    ops::Range,
    path::{Path, PathBuf},
    process::Stdio,
};

use deltaflens_core::{
    Entry, EntryKind, History, Sort, SortKey, SortOrder, external_command, list_entries, platform,
    png_for_clipboard, run_with_input, sort_entries,
};
use eyre::eyre;
use gpui::{
    AnyElement, App, ClickEvent, ClipboardItem, Context, Entity, EventEmitter, MouseButton,
    MouseDownEvent, NavigationDirection, ObjectFit, Pixels, Point, Rgba, ScrollStrategy,
    SharedString, Task, UniformListScrollHandle, Window, anchored, container_query, deferred, div,
    img, prelude::*, px, uniform_list,
};

use crate::{
    settings::SettingsStore,
    theme::{Theme, theme},
    thumbnails::{ThumbnailKey, Thumbnails},
};

/// How much wider and taller a grid cell, including its padding and the name
/// of the entry, is than its icon.
const CELL_EXTRA_WIDTH: f32 = 24.;
const CELL_EXTRA_HEIGHT: f32 = 40.;
/// The space between the grid and the edges of the explorer.
const GRID_PADDING: f32 = 8.;

/// An image being dragged from the explorer, to open it in a pane.
pub struct DraggedImage {
    pub path: PathBuf,
}

/// The name of the dragged image, following the mouse.
struct DraggedImagePreview {
    name: SharedString,
}

impl Render for DraggedImagePreview {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .px_3()
            .py_1()
            .rounded_sm()
            .bg(theme.element)
            .text_sm()
            .text_color(theme.text_strong)
            .child(self.name.clone())
    }
}

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
    settings: Entity<SettingsStore>,
    /// Whether hidden entries are listed, as last read from the settings.
    show_hidden: bool,
    context_menu: Option<ContextMenu>,
    /// The error of the last action from the context menu.
    notice: Option<SharedString>,
}

/// The menu shown by right-clicking an entry.
struct ContextMenu {
    path: PathBuf,
    kind: EntryKind,
    /// In window coordinates.
    position: Point<Pixels>,
}

impl EventEmitter<ExplorerEvent> for Explorer {}

impl Explorer {
    pub fn new(
        dir: PathBuf,
        thumbnails: Entity<Thumbnails>,
        settings: Entity<SettingsStore>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Redraw when a thumbnail is ready.
        cx.observe(&thumbnails, |_, _, cx| cx.notify()).detach();
        cx.observe(&settings, |this, settings, cx| {
            let show_hidden = settings.read(cx).settings().show_hidden_files;
            if show_hidden != this.show_hidden {
                this.show_hidden = show_hidden;
                this.load(cx);
            }
            cx.notify();
        })
        .detach();
        let show_hidden = settings.read(cx).settings().show_hidden_files;
        let mut this = Self {
            history: History::new(dir),
            sort: Sort::default(),
            entries: Vec::new(),
            error: None,
            load_task: None,
            scroll_handle: UniformListScrollHandle::new(),
            thumbnails,
            settings,
            show_hidden,
            context_menu: None,
            notice: None,
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
        self.context_menu = None;
        self.notice = None;
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
        let show_hidden = self.show_hidden;
        self.load_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let mut entries = list_entries(&dir, show_hidden)?;
                    sort_entries(&mut entries, sort);
                    eyre::Ok(entries)
                })
                .await;
            // Fails only if the explorer is gone.
            let _ = this.update(cx, |this, cx| {
                this.load_task = None;
                match result {
                    Ok(mut entries) => {
                        // The sort may have changed while loading.
                        if this.sort != sort {
                            sort_entries(&mut entries, this.sort);
                        }
                        this.entries = entries;
                    }
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn set_sort(&mut self, sort: Sort, cx: &mut Context<Self>) {
        if sort != self.sort {
            self.sort = sort;
            sort_entries(&mut self.entries, sort);
            cx.notify();
        }
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

    fn copy_path(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.context_menu = None;
        // Clipboards hold text, so a path that is not UTF-8 cannot be copied as is.
        cx.write_to_clipboard(ClipboardItem::new_string(
            path.to_string_lossy().into_owned(),
        ));
        cx.notify();
    }

    /// The size of the square that holds the icon of an entry, in logical
    /// pixels.
    fn icon_size(&self, cx: &App) -> f32 {
        // Thumbnail sizes are small, so f32 holds them exactly.
        #[expect(clippy::cast_precision_loss)]
        let size = self.settings.read(cx).settings().thumbnail_size as f32;
        size
    }

    /// The command that opens entries with another application.
    fn open_command(&self, cx: &App) -> String {
        self.settings.read(cx).settings().open_command.clone()
    }

    /// Opens `path` with the command of the settings, without waiting for it
    /// to exit.
    fn open_externally(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.context_menu = None;
        let open_command = self.open_command(cx);
        let spawned = external_command(&open_command, path)
            .and_then(|mut command| Ok(command.stdin(Stdio::null()).spawn()?));
        match spawned {
            Ok(mut child) => {
                self.notice = None;
                // Reap the process when it exits, which may take as long as the
                // application stays open.
                std::thread::spawn(move || child.wait());
            }
            Err(error) => {
                let message = format!("Failed to run \"{open_command}\": {error:#}");
                tracing::error!("{message}");
                self.notice = Some(message.into());
            }
        }
        cx.notify();
    }

    /// Copies the image to the clipboard in the background, with the command
    /// of the system.
    fn copy_image(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.context_menu = None;
        let path = path.to_owned();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let png = png_for_clipboard(&path)?;
                    let command = platform::clipboard_command(|name| std::env::var_os(name))
                        .ok_or_else(|| eyre!("no Wayland or X11 display"))?;
                    run_with_input(command, &png)
                })
                .await;
            // Fails only if the explorer is gone.
            let _ = this.update(cx, |this, cx| {
                this.notice = result.err().map(|error| {
                    let message = format!("Failed to copy the image: {error:#}");
                    tracing::error!("{message}");
                    message.into()
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The menu of an entry. `open_command` opens it with another
    /// application.
    fn render_context_menu(
        menu: &ContextMenu,
        open_command: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let program = open_command
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_owned();
        let theme = theme(cx);
        let item = |id: &'static str, label: String| {
            div()
                .id(id)
                .px_3()
                .py_1()
                .rounded_sm()
                .hover(move |style| style.bg(theme.element_hover))
                .child(label)
        };
        let open_path = menu.path.clone();
        let copy_path = menu.path.clone();
        let image_path = menu.path.clone();
        div()
            .flex()
            .flex_col()
            .min_w(px(180.))
            .p_1()
            .rounded_md()
            .border_1()
            .border_color(theme.menu_border)
            .bg(theme.menu)
            .text_sm()
            .text_color(theme.text)
            .shadow_md()
            .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                this.context_menu = None;
                cx.notify();
            }))
            .child(
                item("open-externally", format!("Open with {program}")).on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| this.open_externally(&open_path, cx),
                )),
            )
            .when(menu.kind == EntryKind::Image, |this| {
                this.child(
                    item("copy-image", "Copy image".to_owned()).on_click(cx.listener(
                        move |this, _: &ClickEvent, _, cx| {
                            this.copy_image(&image_path, cx);
                        },
                    )),
                )
            })
            .child(item("copy-path", "Copy path".to_owned()).on_click(
                cx.listener(move |this, _: &ClickEvent, _, cx| this.copy_path(&copy_path, cx)),
            ))
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(px(36.))
            .px_2()
            .bg(theme.toolbar)
            .border_b_1()
            .border_color(theme.border)
            .child(nav_button(
                "back",
                "←",
                self.history.can_go_back(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_back(cx)),
                &theme,
            ))
            .child(nav_button(
                "forward",
                "→",
                self.history.can_go_forward(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_forward(cx)),
                &theme,
            ))
            .child(nav_button(
                "up",
                "↑",
                self.dir().parent().is_some(),
                cx.listener(|this, _: &ClickEvent, _, cx| this.go_up(cx)),
                &theme,
            ))
            .child(self.render_path_bar(cx))
            .child(self.render_sort_buttons(cx))
    }

    /// Each folder from the root to the current one, as a button that moves
    /// there.
    fn render_path_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut ancestors: Vec<&Path> = self.dir().ancestors().collect();
        ancestors.reverse();
        let last = ancestors.len().saturating_sub(1);
        let theme = theme(cx);
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
                        this.child(div().px_0p5().text_color(theme.text_disabled).child("›"))
                    })
                    .child(
                        div()
                            .id(("path", index))
                            .px_1p5()
                            .py_0p5()
                            .rounded_sm()
                            .text_color(if index == last {
                                theme.text_strong
                            } else {
                                theme.text_muted
                            })
                            .hover(move |style| style.bg(theme.element))
                            .child(name)
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.navigate(target.clone(), cx);
                            })),
                    )
            }))
    }

    /// Chooses the sort key, and toggles the order.
    fn render_sort_buttons(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let sort = self.sort;
        let theme = theme(cx);
        let key_button = |key: SortKey, label: &'static str, cx: &mut Context<Self>| {
            toolbar_button(label, sort.key == key, &theme)
                .id(label)
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.set_sort(Sort { key, ..sort }, cx);
                }))
        };
        let (order_label, reversed) = match sort.order {
            SortOrder::Ascending => ("↑ Ascending", SortOrder::Descending),
            SortOrder::Descending => ("↓ Descending", SortOrder::Ascending),
        };
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .ml_2()
            .text_sm()
            .child(key_button(SortKey::Name, "Name", cx))
            .child(key_button(SortKey::Modified, "Modified", cx))
            .child(
                toolbar_button(order_label, false, &theme)
                    .id("order")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.set_sort(
                            Sort {
                                order: reversed,
                                ..sort
                            },
                            cx,
                        );
                    })),
            )
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
            .text_color(theme(cx).text_placeholder)
            .children(message)
            .into_any_element()
    }

    /// Lays the entries out in rows that fill the width, and renders only the
    /// visible rows.
    fn render_entries(&self, cx: &mut Context<Self>) -> AnyElement {
        let explorer = cx.entity();
        let count = self.entries.len();
        let scroll_handle = self.scroll_handle.clone();
        let cell_width = self.icon_size(cx) + CELL_EXTRA_WIDTH;
        container_query(move |size, _, _| {
            let columns = column_count(size.width, cell_width);
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
        let icon_size = self.icon_size(cx);
        // Truncation is intended; the value is small and positive.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let thumbnail_size = (icon_size * window.scale_factor()).round() as u32;
        rows.map(|row| {
            let start = row * columns;
            let end = (start + columns).min(self.entries.len());
            div().flex().px(px(GRID_PADDING)).children(
                (start..end).map(|index| self.render_cell(index, icon_size, thumbnail_size, cx)),
            )
        })
        .collect()
    }

    fn render_cell(
        &self,
        index: usize,
        icon_size: f32,
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
        let name_for_preview = name.clone();
        let theme = theme(cx);
        let icon = match entry.kind {
            EntryKind::Folder => folder_icon(icon_size, theme.folder).into_any_element(),
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
                    None => image_placeholder(&entry.path, icon_size, &theme).into_any_element(),
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
            .w(px(icon_size + CELL_EXTRA_WIDTH))
            .h(px(icon_size + CELL_EXTRA_HEIGHT))
            .p_1()
            .rounded_md()
            .hover(move |style| style.bg(theme.element))
            .child(
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(icon_size))
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
                    .text_color(theme.text)
                    .child(name),
            )
            // A double click opens the entry. A single click does nothing
            // until entries can be selected.
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                if event.click_count() == 2 {
                    this.open_entry(index, cx);
                }
            }))
            .when(entry.kind == EntryKind::Image, |this| {
                let dragged = DraggedImage {
                    path: entry.path.clone(),
                };
                let name = SharedString::from(name_for_preview);
                this.on_drag(dragged, move |_, _, _, cx| {
                    let name = name.clone();
                    cx.new(|_| DraggedImagePreview { name })
                })
            })
            .on_mouse_down(MouseButton::Right, {
                let path = entry.path.clone();
                let kind = entry.kind;
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.context_menu = Some(ContextMenu {
                        path: path.clone(),
                        kind,
                        position: event.position,
                    });
                    cx.notify();
                })
            })
    }
}

impl Render for Explorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let open_command = self.open_command(cx);
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
            .when_some(self.notice.clone(), |this, notice| {
                this.child(
                    div()
                        .flex_shrink_0()
                        .px_3()
                        .py_1()
                        .bg(theme(cx).error_background)
                        .text_sm()
                        .text_color(theme(cx).error_text)
                        .child(notice),
                )
            })
            .child(div().flex_1().min_h_0().child(self.render_grid(cx)))
            .when_some(self.context_menu.as_ref(), |this, menu| {
                this.child(
                    deferred(
                        anchored()
                            .position(menu.position)
                            .snap_to_window()
                            .child(Self::render_context_menu(menu, &open_command, cx)),
                    )
                    .with_priority(1),
                )
            })
    }
}

/// How many cells `cell_width` wide fit in a row of `width`, at least one.
fn column_count(width: Pixels, cell_width: f32) -> usize {
    let available = f32::from(width) - 2. * GRID_PADDING;
    // Truncation is intended; the value is small and non-negative.
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let columns = (available / cell_width).floor().max(1.) as usize;
    columns
}

/// A text button in the toolbar, highlighted when `selected`.
fn toolbar_button(label: &'static str, selected: bool, theme: &Theme) -> gpui::Div {
    let hover = theme.element;
    div()
        .px_2()
        .py_0p5()
        .rounded_sm()
        .bg(if selected {
            theme.element_hover
        } else {
            theme.toolbar
        })
        .text_color(if selected {
            theme.text_strong
        } else {
            theme.text_muted
        })
        .hover(move |style| style.bg(hover))
        .child(label)
}

fn nav_button(
    id: &'static str,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static,
    theme: &Theme,
) -> impl IntoElement {
    let hover = theme.element;
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(26.))
        .rounded_sm()
        .text_color(if enabled {
            theme.text
        } else {
            theme.text_disabled
        })
        .when(enabled, |this| {
            this.hover(move |style| style.bg(hover)).on_click(on_click)
        })
        .child(label)
}

/// A folder drawn with plain shapes in `color`, so that no icon font is
/// needed, for an icon of `size`.
fn folder_icon(size: f32, color: Rgba) -> impl IntoElement {
    // Drawn for 128 pixels.
    let scale = size / 128.;
    div()
        .flex()
        .flex_col()
        .w(px(104. * scale))
        .child(
            div()
                .w(px(40. * scale))
                .h(px(10. * scale))
                .rounded_t_md()
                .bg(color),
        )
        .child(
            div()
                .w_full()
                .h(px(74. * scale))
                .rounded_md()
                .rounded_tl_none()
                .bg(color),
        )
}

/// Stands in for the thumbnail until it is ready, or if the image cannot be
/// decoded: the extension of the image in a frame, for an icon of `size`.
fn image_placeholder(path: &Path, size: f32, theme: &Theme) -> impl IntoElement {
    let extension = path
        .extension()
        .unwrap_or_default()
        .to_string_lossy()
        .to_uppercase();
    div()
        .flex()
        .items_center()
        .justify_center()
        .size(px(size * 0.75))
        .rounded_md()
        .border_1()
        .border_color(theme.placeholder_border)
        .bg(theme.placeholder)
        .text_xs()
        .text_color(theme.text_muted)
        .child(extension)
}
