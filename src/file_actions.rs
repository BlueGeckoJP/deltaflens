//! What the menus of the explorer and the viewer do with a file, and how they
//! tell about errors.

use std::{path::Path, process::Stdio};

use deltaflens_core::{external_command, platform, png_for_clipboard, run_with_input};
use eyre::{Result, WrapErr as _, eyre};
use gpui::{App, ClipboardItem, SharedString, Task, div, prelude::*};

use crate::theme::theme;

/// The label of the item that opens a file with `open_command`, naming its
/// program.
pub fn open_with_label(open_command: &str) -> String {
    let program = open_command.split_whitespace().next().unwrap_or_default();
    format!("Open with {program}")
}

/// Opens `path` with `open_command`, without waiting for it to exit.
pub fn open_externally(path: &Path, open_command: &str) -> Result<()> {
    let mut child = external_command(open_command, path)
        .and_then(|mut command| Ok(command.stdin(Stdio::null()).spawn()?))
        .wrap_err_with(|| format!("Failed to run \"{open_command}\""))?;
    // Reap the process when it exits, which may take as long as the
    // application stays open.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Copies the image to the clipboard in the background, with the command of
/// the system.
pub fn copy_image(path: &Path, cx: &App) -> Task<Result<()>> {
    let path = path.to_owned();
    cx.background_spawn(async move {
        let copy = || {
            let png = png_for_clipboard(&path)?;
            let command = platform::clipboard_command(|name| std::env::var_os(name))
                .ok_or_else(|| eyre!("no Wayland or X11 display"))?;
            run_with_input(command, &png)
        };
        copy().wrap_err("Failed to copy the image")
    })
}

/// Copies `path` to the clipboard as text.
pub fn copy_path(path: &Path, cx: &mut App) {
    // Clipboards hold text, so a path that is not UTF-8 cannot be copied as is.
    cx.write_to_clipboard(ClipboardItem::new_string(
        path.to_string_lossy().into_owned(),
    ));
}

/// The message to show for the result of an action: its error, if any.
pub fn notice(result: Result<()>) -> Option<SharedString> {
    result.err().map(|error| {
        let message = format!("{error:#}");
        tracing::error!("{message}");
        message.into()
    })
}

/// A bar that shows the error of the last action.
pub fn render_notice(notice: SharedString, cx: &App) -> impl IntoElement + use<> {
    let theme = theme(cx);
    div()
        .flex_shrink_0()
        .px_3()
        .py_1()
        .bg(theme.error_background)
        .text_sm()
        .text_color(theme.error_text)
        .child(notice)
}
