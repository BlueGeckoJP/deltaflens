mod viewer;
mod workspace_view;

use std::path::PathBuf;

use eyre::Result;
use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, prelude::*, px, size,
};
use tracing_subscriber::EnvFilter;

use crate::workspace_view::{NextImage, PreviousImage, ResetView, WorkspaceView};

fn main() -> Result<()> {
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    // Replaced by `--open` once opening files is implemented.
    let path = std::env::args_os().nth(1).map(PathBuf::from);

    gpui_platform::application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("ctrl-0", ResetView, None),
            KeyBinding::new("left", PreviousImage, None),
            KeyBinding::new("right", NextImage, None),
        ]);

        let options = WindowOptions {
            titlebar: Some(TitlebarOptions {
                title: Some(workspace_view::APP_NAME.into()),
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1280.), px(800.)),
                cx,
            ))),
            ..Default::default()
        };
        let result = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let mut view = WorkspaceView::new(window, cx);
                if let Some(path) = path
                    && let Err(error) = view.open_in_new_tab(path, window, cx)
                {
                    tracing::error!("{error:#}");
                }
                view
            })
        });
        if let Err(error) = result {
            tracing::error!("failed to open window: {error:#}");
            cx.quit();
        }
    });

    Ok(())
}
