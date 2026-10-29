//! The window frame drawn by the app (client-side decorations): the shadow,
//! rounded corners and resize edges around the window, and the window buttons.

use gpui::{
    AnyElement, App, Bounds, BoxShadow, CursorStyle, Decorations, Div, MouseButton, MouseDownEvent,
    PathBuilder, Pixels, ResizeEdge, Rgba, Tiling, Window, canvas, div, point, prelude::*, px,
};

use crate::theme::theme;

/// The width of the transparent margin around a floating window that holds
/// the shadow and the resize edges.
const SHADOW_SIZE: Pixels = px(10.);
/// The radius of the corners of a floating window.
pub const ROUNDING: Pixels = px(10.);
/// The width of a window button. Its height is that of the tab bar.
const BUTTON_WIDTH: Pixels = px(46.);
/// The size of the square that holds the icon of a window button.
const ICON_SIZE: Pixels = px(12.);

/// The sides of the window that touch a screen edge or another window, or
/// `None` when the compositor draws the frame.
pub fn client_tiling(window: &Window) -> Option<Tiling> {
    match window.window_decorations() {
        Decorations::Server => None,
        Decorations::Client { tiling } => Some(tiling),
    }
}

/// Wraps `content` in the window frame.
///
/// When the compositor draws the frame, only the background is drawn, so the
/// window looks the same as without client-side decorations.
pub fn window_frame(content: impl IntoElement, window: &mut Window, cx: &App) -> AnyElement {
    let theme = theme(cx);
    let Some(tiling) = client_tiling(window) else {
        window.set_client_inset(px(0.));
        return div()
            .size_full()
            .bg(theme.background)
            .child(content)
            .into_any_element();
    };
    window.set_client_inset(SHADOW_SIZE);
    let resizable = window.is_resizable();
    div()
        .size_full()
        .relative()
        .when(!tiling.top, |this| this.pt(SHADOW_SIZE))
        .when(!tiling.bottom, |this| this.pb(SHADOW_SIZE))
        .when(!tiling.left, |this| this.pl(SHADOW_SIZE))
        .when(!tiling.right, |this| this.pr(SHADOW_SIZE))
        .child(
            div()
                .size_full()
                .bg(theme.background)
                .map(|this| round_corners(this, tiling))
                // A fully tiled window has no margin left to draw the shadow in.
                .when(tiling != Tiling::tiled(), |this| {
                    this.shadow(vec![
                        BoxShadow::new(px(0.), px(0.), theme.shadow).blur_radius(SHADOW_SIZE / 2.),
                    ])
                })
                .child(content),
        )
        .when(resizable, |this| this.children(resize_handles(tiling)))
        .into_any_element()
}

/// Rounds the corners whose two sides both float.
fn round_corners(div: Div, tiling: Tiling) -> Div {
    div.when(!tiling.top && !tiling.left, |this| {
        this.rounded_tl(ROUNDING)
    })
    .when(!tiling.top && !tiling.right, |this| {
        this.rounded_tr(ROUNDING)
    })
    .when(!tiling.bottom && !tiling.left, |this| {
        this.rounded_bl(ROUNDING)
    })
    .when(!tiling.bottom && !tiling.right, |this| {
        this.rounded_br(ROUNDING)
    })
}

/// The edges that resize the window: each floating side, and each corner
/// between two floating sides. The corners come last, so that their handles
/// are drawn on top of the sides.
fn resize_edges(tiling: Tiling) -> Vec<ResizeEdge> {
    [
        (ResizeEdge::Top, !tiling.top),
        (ResizeEdge::Bottom, !tiling.bottom),
        (ResizeEdge::Left, !tiling.left),
        (ResizeEdge::Right, !tiling.right),
        (ResizeEdge::TopLeft, !tiling.top && !tiling.left),
        (ResizeEdge::TopRight, !tiling.top && !tiling.right),
        (ResizeEdge::BottomLeft, !tiling.bottom && !tiling.left),
        (ResizeEdge::BottomRight, !tiling.bottom && !tiling.right),
    ]
    .into_iter()
    .filter_map(|(edge, floating)| floating.then_some(edge))
    .collect()
}

/// The areas in the margin that resize the window.
fn resize_handles(tiling: Tiling) -> Vec<AnyElement> {
    resize_edges(tiling)
        .into_iter()
        .map(resize_handle)
        .collect()
}

fn resize_handle(edge: ResizeEdge) -> AnyElement {
    let handle = div().absolute();
    let handle = match edge {
        ResizeEdge::Top => handle.top_0().left_0().w_full().h(SHADOW_SIZE),
        ResizeEdge::Bottom => handle.bottom_0().left_0().w_full().h(SHADOW_SIZE),
        ResizeEdge::Left => handle.top_0().left_0().h_full().w(SHADOW_SIZE),
        ResizeEdge::Right => handle.top_0().right_0().h_full().w(SHADOW_SIZE),
        ResizeEdge::TopLeft => handle.top_0().left_0().size(SHADOW_SIZE),
        ResizeEdge::TopRight => handle.top_0().right_0().size(SHADOW_SIZE),
        ResizeEdge::BottomLeft => handle.bottom_0().left_0().size(SHADOW_SIZE),
        ResizeEdge::BottomRight => handle.bottom_0().right_0().size(SHADOW_SIZE),
    };
    let cursor = match edge {
        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
        ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
        ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
    };
    handle
        .cursor(cursor)
        .on_mouse_down(MouseButton::Left, move |_: &MouseDownEvent, window, _| {
            window.start_window_resize(edge);
        })
        .into_any_element()
}

#[derive(Clone, Copy)]
enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// The minimize, maximize (or restore) and close buttons. Only the buttons the
/// compositor supports are shown.
///
/// `round_top_right` rounds the close button to fit the corner of the window.
pub fn window_controls(window: &Window, round_top_right: bool, cx: &App) -> impl IntoElement {
    let theme = theme(cx);
    let controls = window.window_controls();
    let zoom_icon = if window.is_maximized() {
        Icon::Restore
    } else {
        Icon::Maximize
    };
    div()
        .flex()
        .flex_shrink_0()
        .h_full()
        .when(controls.minimize, |this| {
            this.child(
                window_button("minimize", Icon::Minimize, theme.element, theme.text)
                    .on_click(|_, window, _| window.minimize_window()),
            )
        })
        .when(controls.maximize, |this| {
            this.child(
                window_button("maximize", zoom_icon, theme.element, theme.text)
                    .on_click(|_, window, _| window.zoom_window()),
            )
        })
        .child(
            window_button("close", Icon::Close, theme.close_hover, theme.close_icon)
                .when(round_top_right, |this| this.rounded_tr(ROUNDING))
                .on_click(|_, window, _| window.remove_window()),
        )
}

/// A window button showing `icon` in `color`, on `hover` under the mouse.
fn window_button(id: &'static str, icon: Icon, hover: Rgba, color: Rgba) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(BUTTON_WIDTH)
        .h_full()
        .hover(move |style| style.bg(hover))
        .child(
            canvas(
                |_, _, _| {},
                move |bounds, (), window, _| paint_icon(icon, color, bounds, window),
            )
            .size(ICON_SIZE),
        )
}

/// Draws `icon` with thin lines of `color` in `bounds`, a square of
/// [`ICON_SIZE`].
fn paint_icon(icon: Icon, color: Rgba, bounds: Bounds<Pixels>, window: &mut Window) {
    // The points are on a 12 × 12 grid, at the centers of the pixels so that
    // the 1 px lines stay sharp.
    let lines: &[[(f32, f32); 2]] = match icon {
        Icon::Minimize => &[[(2., 6.5), (10., 6.5)]],
        Icon::Maximize => &[
            [(2.5, 2.5), (9.5, 2.5)],
            [(9.5, 2.5), (9.5, 9.5)],
            [(9.5, 9.5), (2.5, 9.5)],
            [(2.5, 9.5), (2.5, 2.5)],
        ],
        Icon::Restore => &[
            // The window in front.
            [(2.5, 4.5), (7.5, 4.5)],
            [(7.5, 4.5), (7.5, 9.5)],
            [(7.5, 9.5), (2.5, 9.5)],
            [(2.5, 9.5), (2.5, 4.5)],
            // The visible edges of the window behind.
            [(4.5, 4.5), (4.5, 2.5)],
            [(4.5, 2.5), (9.5, 2.5)],
            [(9.5, 2.5), (9.5, 7.5)],
            [(9.5, 7.5), (7.5, 7.5)],
        ],
        Icon::Close => &[[(2.5, 2.5), (9.5, 9.5)], [(9.5, 2.5), (2.5, 9.5)]],
    };
    let origin = bounds.origin;
    let mut builder = PathBuilder::stroke(px(1.));
    for [(x0, y0), (x1, y1)] in lines {
        builder.move_to(origin + point(px(*x0), px(*y0)));
        builder.line_to(origin + point(px(*x1), px(*y1)));
    }
    match builder.build() {
        Ok(path) => window.paint_path(path, color),
        Err(error) => tracing::error!("failed to draw a window button: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_window_resizes_from_every_edge() {
        assert_eq!(
            resize_edges(Tiling::default()),
            [
                ResizeEdge::Top,
                ResizeEdge::Bottom,
                ResizeEdge::Left,
                ResizeEdge::Right,
                ResizeEdge::TopLeft,
                ResizeEdge::TopRight,
                ResizeEdge::BottomLeft,
                ResizeEdge::BottomRight,
            ]
        );
    }

    #[test]
    fn tiled_window_does_not_resize() {
        assert_eq!(resize_edges(Tiling::tiled()), []);
    }

    #[test]
    fn window_tiled_on_the_left_resizes_from_the_other_sides() {
        let tiling = Tiling {
            top: true,
            left: true,
            bottom: true,
            right: false,
        };
        assert_eq!(resize_edges(tiling), [ResizeEdge::Right]);
    }

    #[test]
    fn window_tiled_at_the_top_keeps_the_bottom_corners() {
        let tiling = Tiling {
            top: true,
            ..Tiling::default()
        };
        assert_eq!(
            resize_edges(tiling),
            [
                ResizeEdge::Bottom,
                ResizeEdge::Left,
                ResizeEdge::Right,
                ResizeEdge::BottomLeft,
                ResizeEdge::BottomRight,
            ]
        );
    }
}
