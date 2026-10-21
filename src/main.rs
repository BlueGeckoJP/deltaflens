use eyre::Result;
use gpui::{
    App, Bounds, Context, TitlebarOptions, Window, WindowBounds, WindowOptions, div, prelude::*,
    px, size,
};
use tracing_subscriber::EnvFilter;

struct RootView;

impl Render for RootView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    gpui_platform::application().run(|cx: &mut App| {
        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some("deltaflens".into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(800.)),
                cx,
            ))),
            ..Default::default()
        };
        if let Err(error) = cx.open_window(options, |_, cx| cx.new(|_| RootView)) {
            tracing::error!("failed to open window: {error:#}");
            cx.quit();
        }
    });

    Ok(())
}
