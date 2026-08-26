use gpui::{
    App, Bounds, Render, RenderImage, Window, WindowOptions, div, img, prelude::*, px, rgb, size,
};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use image::{Frame, ImageFormat};
use rfd::FileDialog;
use std::sync::{Arc, LazyLock};
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
}

impl Render for MainApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();

        let image_content = match &self.image {
            ImageState::Empty => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child("No image")
                .into_any_element(),
            ImageState::Loading => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child("Loading...")
                .into_any_element(),
            ImageState::Loaded(image) => img(image.clone()).into_any_element(),
            ImageState::Error(e) => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child(e.to_owned())
                .into_any_element(),
        };

        div()
            .bg(rgb(0xffffff))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .context_menu(move |menu, _window, _cx| {
                let this = this.clone();

                menu.item(PopupMenuItem::new("Open").on_click(move |_, _, cx| {
                    let Some(path) = FileDialog::new()
                        .add_filter("Images", &SUPPORTED_EXTENSIONS)
                        .pick_file()
                    else {
                        return;
                    };

                    this.update(cx, |app, cx| {
                        app.image = ImageState::Loading;
                        cx.notify();
                    });

                    let task = cx.background_spawn(async move {
                        let mut rgba = image::open(path)?.into_rgba8();

                        for pixel in rgba.pixels_mut() {
                            pixel.0.swap(0, 2);
                        }

                        let frame = Frame::new(rgba);

                        eyre::Ok(Arc::new(RenderImage::new([frame])))
                    });

                    let this = this.clone();

                    cx.spawn(async move |cx| match task.await {
                        Ok(render_image) => {
                            this.update(cx, |app, cx| {
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
                }))
                .item(PopupMenuItem::Separator)
                .item(PopupMenuItem::new("Close").on_click(|_, _, cx| {
                    cx.quit();
                }))
            })
            .child(image_content)
    }
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
            |_, cx| {
                cx.new(|_| MainApp {
                    image: ImageState::Empty,
                })
            },
        )
        .unwrap();
    });
}
