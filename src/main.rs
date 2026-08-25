use gpui::{App, Bounds, Render, Window, WindowOptions, div, prelude::*, px, rgb, size};
use gpui_component::menu::{ContextMenuExt, PopupMenuItem};

struct MainApp {}

impl Render for MainApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(0xffffff))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .context_menu(|menu, _window, _cx| {
                menu.item(PopupMenuItem::new("Open").on_click(|_, _, _cx| {
                    println!("Open");
                }))
                .item(PopupMenuItem::new("Close").on_click(|_, _, cx| {
                    cx.quit();
                }))
            })
            .child("Hello, world!")
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
            |_, cx| cx.new(|_| MainApp {}),
        )
        .unwrap();
    });
}
