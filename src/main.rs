use gpui::{
    AnyElement, App, Bounds, Entity, FocusHandle, KeyUpEvent, MouseDownEvent, MouseMoveEvent,
    Pixels, Point, Render, RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Size, Window,
    WindowOptions, div, img, prelude::*, px, rgb, size,
};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use image::{Frame, ImageFormat};
use rfd::FileDialog;
use std::{
    path::PathBuf,
    sync::{Arc, LazyLock},
};
use tracing::error;
use tracing_subscriber::EnvFilter;

static SUPPORTED_EXTENSIONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ImageFormat::all()
        .filter(|f| f.reading_enabled())
        .flat_map(ImageFormat::extensions_str)
        .copied()
        .collect()
});

#[derive(Clone, Copy)]
enum ImageNavigation {
    Previous,
    Next,
}

#[derive(Debug, Default)]
enum ImageState {
    #[default]
    Empty,
    Loading,
    Loaded(Arc<RenderImage>),
    Error(String),
}

#[derive(Debug, Default)]
enum ImageSelection {
    #[default]
    None,
    Standalone {
        #[allow(dead_code)]
        path: PathBuf,
    },
    Directory {
        #[allow(dead_code)]
        directory: PathBuf,
        images: Vec<PathBuf>,
        current_index: usize,
    },
}

struct MainApp {
    focus_handle: FocusHandle,

    image: ImageState,
    selection: ImageSelection,
    open_generation: u64,

    zoom: f32,
    image_offset: Point<Pixels>,
    last_mouse_position: Option<Point<Pixels>>,
}

impl MainApp {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),

            image: ImageState::default(),
            selection: ImageSelection::default(),
            open_generation: 0,

            zoom: 1.0,
            image_offset: Point::default(),
            last_mouse_position: None,
        }
    }

    fn spawn_decode(
        this: Entity<Self>,
        cx: &mut App,
        path: PathBuf,
        generation: u64,
        window_size: Size<Pixels>,
        scale_factor: f32,
    ) {
        let task = cx.background_spawn(decode_image(path));

        cx.spawn(async move |cx| {
            let result = task.await;

            this.update(cx, |app, cx| {
                if app.open_generation != generation {
                    return;
                }

                match result {
                    Ok(render_image) => {
                        app.zoom = calculate_min_zoom(&render_image, window_size, scale_factor);
                        app.image = ImageState::Loaded(render_image);
                    }

                    Err(e) => {
                        error!("Failed to open image: {e}");
                        app.image = ImageState::Error(e.to_string());
                    }
                }

                cx.notify();
            });
        })
        .detach();
    }

    fn open_image(this: Entity<Self>, window: &Window, cx: &mut App, path: PathBuf) {
        let path = match make_absolute(path) {
            Ok(path) => path,
            Err(e) => {
                error!("Failed to make path absolute: {e}");
                return;
            }
        };
        let window_size = window.bounds().size;
        let scale_factor = window.scale_factor();

        let generation = this.update(cx, |app, cx| {
            app.open_generation = app.open_generation.wrapping_add(1);

            app.image = ImageState::Loading;
            app.selection = ImageSelection::Standalone { path: path.clone() };

            app.zoom = 1.0;
            app.image_offset = Point::default();
            app.last_mouse_position = None;

            cx.notify();
            app.open_generation
        });

        Self::spawn_decode(
            this.clone(),
            cx,
            path.clone(),
            generation,
            window_size,
            scale_factor,
        );

        let scan_path = path.clone();
        let scan_task = cx.background_spawn(async move { MainApp::scan_directory(scan_path) });

        cx.spawn(async move |cx| {
            let result = scan_task.await;

            this.update(cx, |app, cx| {
                if app.open_generation != generation {
                    return;
                }

                match result {
                    Ok(Some(selection)) => {
                        app.selection = selection;
                        cx.notify();
                    }
                    Ok(None) => {}
                    Err(e) => error!("Failed to scan image directory: {e}"),
                }
            });
        })
        .detach();
    }

    fn scan_directory(selected_path: PathBuf) -> eyre::Result<Option<ImageSelection>> {
        let Some(directory) = selected_path.parent() else {
            return Ok(None);
        };

        let mut images = Vec::new();

        for entry in std::fs::read_dir(directory)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    error!("Failed to read directory entry: {e}");
                    continue;
                }
            };

            let path = entry.path();

            let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
                continue;
            };

            if !SUPPORTED_EXTENSIONS
                .iter()
                .any(|supported| extension.eq_ignore_ascii_case(supported))
            {
                continue;
            }

            match std::fs::metadata(&path) {
                Ok(metadata) if metadata.is_file() => images.push(path),
                Ok(_) => {}
                Err(e) => {
                    error!("Failed to inspect {path:?}: {e}");
                }
            }
        }

        images.sort_by(|a, b| natord::compare(&a.to_string_lossy(), &b.to_string_lossy()));

        let Some(current_index) = images.iter().position(|path| path == &selected_path) else {
            return Ok(None);
        };

        Ok(Some(ImageSelection::Directory {
            directory: directory.to_path_buf(),
            images,
            current_index,
        }))
    }

    fn handle_image_zoom_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ImageState::Loaded(image) = &self.image else {
            return;
        };

        let min_zoom = calculate_min_zoom(image, window.bounds().size, window.scale_factor());
        let max_zoom = 10.0;

        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y),
            ScrollDelta::Lines(delta) => delta.y * 20.0,
        };
        let factor = if delta > 0.0 { 1.1 } else { 1.0 / 1.1 };

        let old_zoom = self.zoom;
        let new_zoom = (self.zoom * factor).clamp(min_zoom, max_zoom);

        if new_zoom != old_zoom {
            let ratio = new_zoom / self.zoom;

            self.image_offset.x *= ratio;
            self.image_offset.y *= ratio;

            self.zoom = new_zoom;

            cx.notify();
        }

        self.zoom = self.zoom.clamp(min_zoom, max_zoom);

        cx.notify();
    }

    fn navigate_image(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
        navigation: ImageNavigation,
    ) {
        let path = match &mut self.selection {
            ImageSelection::Directory {
                images,
                current_index,
                ..
            } if images.len() > 1 => {
                *current_index = match navigation {
                    ImageNavigation::Previous => {
                        if *current_index == 0 {
                            images.len() - 1
                        } else {
                            *current_index - 1
                        }
                    }

                    ImageNavigation::Next => (*current_index + 1) % images.len(),
                };

                images[*current_index].clone()
            }
            _ => return,
        };

        self.open_generation = self.open_generation.wrapping_add(1);
        let generation = self.open_generation;

        self.image = ImageState::Loading;
        self.zoom = 1.0;
        self.image_offset = Point::default();
        self.last_mouse_position = None;

        cx.notify();

        let this = cx.entity();

        Self::spawn_decode(
            this,
            cx,
            path,
            generation,
            window.bounds().size,
            window.scale_factor(),
        );
    }
}

impl Render for MainApp {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();

        let image_content = match &self.image {
            ImageState::Empty => centered_message("No image"),
            ImageState::Loading => centered_message("Loading..."),
            ImageState::Loaded(image) => {
                let image_size = image.size(0).to_pixels(window.scale_factor());

                let scaled_width = image_size.width.as_f32() * self.zoom;
                let scaled_height = image_size.height.as_f32() * self.zoom;

                let viewport_size = window.viewport_size();
                let left = (viewport_size.width.as_f32() - scaled_width) / 2.0
                    + self.image_offset.x.as_f32();
                let top = (viewport_size.height.as_f32() - scaled_height) / 2.0
                    + self.image_offset.y.as_f32();

                div()
                    .size_full()
                    .flex()
                    .justify_center()
                    .items_center()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(Self::handle_image_zoom_scroll))
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, _| {
                            this.last_mouse_position = Some(event.position);
                        }),
                    )
                    .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button != Some(gpui::MouseButton::Left) {
                            return;
                        }

                        let Some(last_position) = this.last_mouse_position else {
                            return;
                        };

                        let delta = event.position - last_position;

                        this.image_offset.x += delta.x;
                        this.image_offset.y += delta.y;

                        this.last_mouse_position = Some(event.position);

                        cx.notify();
                    }))
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            this.last_mouse_position = None;
                        }),
                    )
                    .child(
                        img(image.clone())
                            .absolute()
                            .left(px(left))
                            .top(px(top))
                            .w(px(scaled_width))
                            .h(px(scaled_height)),
                    )
                    .into_any_element()
            }
            ImageState::Error(e) => centered_message(e.to_owned()),
        };

        div()
            .track_focus(&self.focus_handle)
            .on_key_up(cx.listener(|app, event: &KeyUpEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "left" => {
                        app.navigate_image(window, cx, ImageNavigation::Previous);
                    }
                    "right" => {
                        app.navigate_image(window, cx, ImageNavigation::Next);
                    }
                    _ => {}
                }
            }))
            .bg(rgb(0xffffff))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .context_menu(move |menu, _window, _cx| {
                let this = this.clone();

                menu.item(PopupMenuItem::new("Open").on_click({
                    let this = this.clone();

                    move |_, window, cx| {
                        let Some(path) = FileDialog::new()
                            .add_filter("Images", &SUPPORTED_EXTENSIONS)
                            .pick_file()
                        else {
                            return;
                        };

                        MainApp::open_image(this.clone(), window, cx, path);
                    }
                }))
                .item(PopupMenuItem::Separator)
                .item(
                    PopupMenuItem::new("Reset Image").on_click(move |_, window, cx| {
                        this.update(cx, |app, cx| {
                            let ImageState::Loaded(image) = &app.image else {
                                return;
                            };

                            app.zoom = calculate_min_zoom(
                                image,
                                window.bounds().size,
                                window.scale_factor(),
                            );
                            app.image_offset = Point::default();

                            cx.notify();
                        });

                        window.refresh();
                    }),
                )
                .item(PopupMenuItem::Separator)
                .item(PopupMenuItem::new("Close").on_click(|_, _, cx| {
                    cx.quit();
                }))
            })
            .child(image_content)
    }
}

fn make_absolute(path: PathBuf) -> eyre::Result<PathBuf> {
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(std::env::current_dir()?.join(path))
    }
}

fn calculate_min_zoom(image: &RenderImage, window_size: Size<Pixels>, scale_factor: f32) -> f32 {
    let image_size = image.size(0).to_pixels(scale_factor);
    let scale_x = window_size.width.as_f32() / image_size.width.as_f32();
    let scale_y = window_size.height.as_f32() / image_size.height.as_f32();
    scale_x.min(scale_y)
}

fn centered_message(message: impl Into<SharedString>) -> AnyElement {
    div()
        .size_full()
        .flex()
        .justify_center()
        .items_center()
        .child(message.into())
        .into_any_element()
}

async fn decode_image(path: PathBuf) -> eyre::Result<Arc<RenderImage>> {
    let mut rgba = image::open(path)?.into_rgba8();

    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    let frame = Frame::new(rgba);

    Ok(Arc::new(RenderImage::new([frame])))
}

fn main() {
    color_eyre::install().unwrap();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let app = gpui_platform::application().with_assets(gpui_component_assets::Assets);

    app.run(|cx: &mut App| {
        // This must be called before using any GPUI Component features.
        gpui_component::init(cx);

        let bounds = Bounds::centered(None, size(px(800.), px(600.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    cx.observe_window_bounds(window, |app: &mut MainApp, window, cx| {
                        let ImageState::Loaded(image) = &app.image else {
                            return;
                        };

                        app.zoom =
                            calculate_min_zoom(image, window.bounds().size, window.scale_factor());
                        cx.notify();
                    })
                    .detach();

                    let app = MainApp::new(cx);
                    window.focus(&app.focus_handle, cx);
                    app
                })
            },
        )
        .unwrap();
    });
}
