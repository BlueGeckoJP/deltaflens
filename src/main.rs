use gpui::{
    App, Application, Bounds, Render, Window, WindowOptions, div, prelude::*, px, rgb, size,
};

struct MainApp {}

impl Render for MainApp {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .bg(rgb(0xffffff))
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child("Hello, world!")
    }
}

fn main() {
    Application::new().run(|cx: &mut App| {
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
