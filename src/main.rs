use gpui::{
    AnyElement, App, Bounds, Entity, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render,
    RenderImage, ScrollDelta, ScrollWheelEvent, SharedString, Size, Window, WindowOptions, div,
    img, point, prelude::*, px, rgb, size,
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

enum ImageState {
    Empty,
    Loading,
    Loaded(Arc<RenderImage>),
    Error(String),
}

struct MainApp {
    image: ImageState,
    zoom: f32,
    image_offset: Point<Pixels>,
    last_mouse_position: Option<Point<Pixels>>,
}

impl MainApp {
    fn open_image(this: Entity<Self>, window: &Window, cx: &mut App, path: PathBuf) {
        this.update(cx, |app, cx| {
            app.image = ImageState::Loading;
            app.zoom = 1.0;
            app.image_offset = Point::default();
            cx.notify()
        });

        let window_size = window.bounds().size;
        let scale_factor = window.scale_factor();

        let task = cx.background_spawn(decode_image(path));

        let this = this.clone();

        cx.spawn(async move |cx| match task.await {
            Ok(render_image) => {
                this.update(cx, |app, cx| {
                    app.zoom = calculate_min_zoom(&render_image, window_size, scale_factor);
                    app.image = ImageState::Loaded(render_image);
                    cx.notify();
                });
            }

            Err(e) => {
                error!("Failed to open image: {e}");

                this.update(cx, |app, cx| {
                    app.image = ImageState::Error(e.to_string());
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn handle_image_zoom_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !event.modifiers.control {
            return;
        }

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
                        div()
                            .size_full()
                            .flex()
                            .justify_center()
                            .items_center()
                            .child(
                                div()
                                    .absolute()
                                    .left(px(left))
                                    .top(px(top))
                                    .w(px(scaled_width))
                                    .h(px(scaled_height))
                                    .child(img(image.clone()).size_full()),
                            ),
                    )
                    .into_any_element()
            }
            ImageState::Error(e) => centered_message(e.to_owned()),
        };

        div()
            .bg(rgb(0xffffff))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .context_menu(move |menu, _window, _cx| {
                let this = this.clone();

                menu.item(PopupMenuItem::new("Open").on_click(move |_, window, cx| {
                    let Some(path) = FileDialog::new()
                        .add_filter("Images", &SUPPORTED_EXTENSIONS)
                        .pick_file()
                    else {
                        return;
                    };

                    MainApp::open_image(this.clone(), window, cx, path);
                }))
                .item(PopupMenuItem::Separator)
                .item(PopupMenuItem::new("Close").on_click(|_, _, cx| {
                    cx.quit();
                }))
            })
            .child(image_content)
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

                    MainApp {
                        image: ImageState::Empty,
                        zoom: 1.0,
                        image_offset: point(px(0.0), px(0.0)),
                        last_mouse_position: None,
                    }
                })
            },
        )
        .unwrap();
    });
}
