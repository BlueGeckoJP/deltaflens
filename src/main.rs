use std::sync::{Arc, LazyLock};

use gpui::{
    App, Bounds, Render, RenderImage, Window, WindowOptions, div, img, prelude::*, px, rgb, size,
};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};
use image::{Frame, ImageFormat};
use rfd::FileDialog;

static SUPPORTED_EXTENSIONS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ImageFormat::all()
        .filter(|f| f.reading_enabled())
        .flat_map(ImageFormat::extensions_str)
        .copied()
        .collect()
});

struct MainApp {
    image: Option<Arc<RenderImage>>,
}

impl Render for MainApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity();

        let image_content = match &self.image {
            Some(image) => img(image.clone()).into_any_element(),
            None => div()
                .size_full()
                .flex()
                .justify_center()
                .items_center()
                .child("No image")
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
                    let file_dialog = FileDialog::new()
                        .add_filter("Images", &SUPPORTED_EXTENSIONS)
                        .pick_file()
                        .unwrap();

                    let mut rgba = image::open(file_dialog).unwrap().into_rgba8();

                    for pixel in rgba.pixels_mut() {
                        pixel.0.swap(0, 2);
                    }

                    let frame = Frame::new(rgba);
                    let render_image = Arc::new(RenderImage::new([frame]));

                    this.update(cx, |app, cx| {
                        app.image = Some(render_image);
                        cx.notify();
                    })
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
            |_, cx| cx.new(|_| MainApp { image: None }),
        )
        .unwrap();
    });
}
