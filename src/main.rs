mod explorer;
mod images;
mod render_image;
mod settings;
mod settings_view;
mod text_input;
mod theme;
mod thumbnails;
mod viewer;
mod window_frame;
mod workspace_view;

use std::path::PathBuf;

use clap::Parser;
use deltaflens_core::platform;
use eyre::Result;
use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBackgroundAppearance, WindowBounds,
    WindowDecorations, WindowOptions, actions, prelude::*, px, size,
};
use tracing_subscriber::EnvFilter;

use crate::workspace_view::{
    ActivateNextTab, ActivatePreviousTab, CloseTab, FocusPaneDown, FocusPaneLeft, FocusPaneRight,
    FocusPaneUp, GoBack, GoForward, GoUp, LayoutExplorerAndViewer, LayoutGrid, LayoutSingle,
    LayoutThreeColumns, LayoutTwoColumns, NewTab, NextImage, Open, OpenSettings, PreviousImage,
    ReopenClosedTab, ResetView, SplitDown, SplitRight, SwapPaneDown, SwapPaneLeft, SwapPaneRight,
    SwapPaneUp, WorkspaceView,
};

actions!(deltaflens, [Quit]);

/// An image viewer centered on JPEG XL.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image or folder to open. Without it, the working directory is opened.
    #[arg(long, value_name = "PATH")]
    open: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    platform::return_large_allocations_to_the_system();

    gpui_platform::application().run(move |cx: &mut App| {
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.bind_keys([
            KeyBinding::new("ctrl-o", Open, None),
            KeyBinding::new("ctrl-n", NewTab, None),
            KeyBinding::new("ctrl-w", CloseTab, None),
            KeyBinding::new("ctrl-shift-t", ReopenClosedTab, None),
            KeyBinding::new("ctrl-tab", ActivateNextTab, None),
            KeyBinding::new("ctrl-shift-tab", ActivatePreviousTab, None),
            KeyBinding::new("ctrl-q", Quit, None),
            KeyBinding::new("ctrl-0", ResetView, None),
            KeyBinding::new("left", PreviousImage, None),
            KeyBinding::new("right", NextImage, None),
            KeyBinding::new("alt-left", GoBack, None),
            KeyBinding::new("alt-right", GoForward, None),
            KeyBinding::new("alt-up", GoUp, None),
            KeyBinding::new("ctrl-k right", SplitRight, None),
            KeyBinding::new("ctrl-k down", SplitDown, None),
            KeyBinding::new("ctrl-alt-left", FocusPaneLeft, None),
            KeyBinding::new("ctrl-alt-right", FocusPaneRight, None),
            KeyBinding::new("ctrl-alt-up", FocusPaneUp, None),
            KeyBinding::new("ctrl-alt-down", FocusPaneDown, None),
            KeyBinding::new("ctrl-alt-shift-left", SwapPaneLeft, None),
            KeyBinding::new("ctrl-alt-shift-right", SwapPaneRight, None),
            KeyBinding::new("ctrl-alt-shift-up", SwapPaneUp, None),
            KeyBinding::new("ctrl-alt-shift-down", SwapPaneDown, None),
            KeyBinding::new("ctrl-k 1", LayoutSingle, None),
            KeyBinding::new("ctrl-k 2", LayoutTwoColumns, None),
            KeyBinding::new("ctrl-k 3", LayoutThreeColumns, None),
            KeyBinding::new("ctrl-k 4", LayoutGrid, None),
            KeyBinding::new("ctrl-k 5", LayoutExplorerAndViewer, None),
            KeyBinding::new("ctrl-,", OpenSettings, None),
        ]);
        cx.bind_keys(text_input::key_bindings());

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
            // Draw the frame in the app, so that the tab bars at the top work as
            // the title bar. The compositor may still draw it instead.
            window_decorations: Some(WindowDecorations::Client),
            // The margin around the window that holds the shadow must be
            // see-through. On X11 an opaque window paints it black.
            window_background: WindowBackgroundAppearance::Transparent,
            ..Default::default()
        };
        let result = cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let mut view = WorkspaceView::new(window, cx);
                let restored = view.restore_session(window, cx);
                // A new session starts in the working directory.
                let path = args
                    .open
                    .or_else(|| (!restored).then(workspace_view::default_dir).flatten());
                if let Some(path) = path {
                    view.open_path(path, window, cx);
                } else if !restored {
                    tracing::error!("no folder to open: the working directory is unknown");
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn args_are_well_formed() {
        Args::command().debug_assert();
    }

    #[test]
    fn open_takes_a_path() {
        let args = Args::try_parse_from(["deltaflens", "--open", "a b.png"]).unwrap();
        assert_eq!(args.open, Some(PathBuf::from("a b.png")));
    }

    #[test]
    fn open_is_optional() {
        let args = Args::try_parse_from(["deltaflens"]).unwrap();
        assert_eq!(args.open, None);
    }

    #[test]
    fn open_without_path_fails() {
        assert!(Args::try_parse_from(["deltaflens", "--open"]).is_err());
    }

    #[test]
    fn positional_path_is_rejected() {
        assert!(Args::try_parse_from(["deltaflens", "a.png"]).is_err());
    }
}
