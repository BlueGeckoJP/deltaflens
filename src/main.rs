mod viewer;
mod workspace_view;

use std::path::PathBuf;

use clap::Parser;
use eyre::Result;
use gpui::{
    App, Bounds, KeyBinding, TitlebarOptions, WindowBounds, WindowOptions, actions, prelude::*, px,
    size,
};
use tracing_subscriber::EnvFilter;

use crate::workspace_view::{NextImage, Open, PreviousImage, ResetView, WorkspaceView};

actions!(deltaflens, [Quit]);

/// An image viewer centered on JPEG XL.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Image to open.
    #[arg(long, value_name = "PATH")]
    open: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    color_eyre::install()?;
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

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
            KeyBinding::new("ctrl-q", Quit, None),
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
                if let Some(path) = args.open
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
