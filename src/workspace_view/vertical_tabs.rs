//! The vertical tabs: a column at the left of the window that lists the tabs
//! of every pane, as a rail of thumbnails or in full with their names.

use deltaflens_core::{PaneId, Rect, Tab, TabContent, TabGroup, VerticalTabs};
use gpui::{
    AnyElement, ClickEvent, Context, ElementId, MouseButton, MouseDownEvent, ObjectFit, Pixels,
    Rgba, SharedString, Window, canvas, div, img, prelude::*, px,
};

use super::{ANCHOR_LINES, DraggedTab, NewTab, TAB_BAR_HEIGHT, WorkspaceView, tab_name};
use crate::{
    explorer::{DraggedImage, folder_icon},
    theme::{Theme, theme},
    thumbnails::ThumbnailKey,
    window_frame::{self, ROUNDING},
};

const RAIL_WIDTH: Pixels = px(52.);
const FULL_WIDTH: Pixels = px(240.);
/// The size of a tile of the rail, in logical pixels.
const TILE_SIZE: f32 = 36.;
/// The ring that marks a tile, around a gap of a pixel.
const TILE_RING_WIDTH: f32 = 2.;
const ROW_HEIGHT: Pixels = px(28.);
const ROW_ICON_SIZE: f32 = 16.;
/// How far the rows are indented, to start under the miniature of their
/// pane.
const ROW_INDENT: Pixels = px(26.);
const GROUP_HEADER_HEIGHT: Pixels = px(26.);
/// The size of the miniature of the layout that marks a pane.
const MINIATURE_WIDTH: f32 = 18.;
const MINIATURE_HEIGHT: f32 = 12.;
/// The size of a square button in the header.
const BUTTON_SIZE: Pixels = px(28.);

/// A window split into a narrow column and the rest, on a 14 × 14 grid: the
/// button that switches between the rail and the full width.
const SIDEBAR_LINES: &[[(f32, f32); 2]] = &[
    [(1.5, 2.5), (12.5, 2.5)],
    [(12.5, 2.5), (12.5, 11.5)],
    [(12.5, 11.5), (1.5, 11.5)],
    [(1.5, 11.5), (1.5, 2.5)],
    [(5.5, 2.5), (5.5, 11.5)],
];
/// A plus on a 12 × 12 grid: the button that opens a new tab.
const PLUS_LINES: &[[(f32, f32); 2]] = &[[(6.5, 1.5), (6.5, 11.5)], [(1.5, 6.5), (11.5, 6.5)]];
/// Chevrons on a 10 × 10 grid: a pane whose tabs are shown, or hidden.
const EXPANDED_LINES: &[[(f32, f32); 2]] = &[[(2., 3.5), (5., 6.5)], [(5., 6.5), (8., 3.5)]];
const COLLAPSED_LINES: &[[(f32, f32); 2]] = &[[(3.5, 2.), (6.5, 5.)], [(6.5, 5.), (3.5, 8.)]];
/// Sliders on a 16 × 16 grid: the settings tab.
const SETTINGS_LINES: &[[(f32, f32); 2]] = &[
    [(2., 4.5), (14., 4.5)],
    [(2., 11.5), (14., 11.5)],
    [(5.5, 2.5), (5.5, 6.5)],
    [(10.5, 9.5), (10.5, 13.5)],
];

/// A short text over an element under the mouse, with an optional detail
/// after it.
struct Tooltip {
    text: SharedString,
    detail: Option<SharedString>,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .flex()
            .gap_2()
            .px_2()
            .py_1()
            .rounded_sm()
            .border_1()
            .border_color(theme.menu_border)
            .bg(theme.menu)
            .text_sm()
            .text_color(theme.text_strong)
            .child(self.text.clone())
            .children(
                self.detail
                    .clone()
                    .map(|detail| div().text_color(theme.text_muted).child(detail)),
            )
    }
}

/// Builds a tooltip of `text` and `detail`, for [`StatefulInteractiveElement::tooltip`].
fn tooltip(
    text: impl Into<SharedString>,
    detail: Option<SharedString>,
) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView {
    let text = text.into();
    move |_, cx| {
        let (text, detail) = (text.clone(), detail.clone());
        cx.new(|_| Tooltip { text, detail }).into()
    }
}

/// `lines` drawn in `color`, in a square of `size`.
fn line_icon(lines: &'static [[(f32, f32); 2]], size: f32, color: Rgba) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |bounds, (), window, _| window_frame::paint_lines(lines, color, bounds, window),
    )
    .flex_shrink_0()
    .size(px(size))
}

/// The layout in small, with the pane `pane_id` light and the others dark.
fn miniature(rects: &[(PaneId, Rect)], pane_id: PaneId, theme: &Theme) -> impl IntoElement {
    div()
        .relative()
        .flex_shrink_0()
        .w(px(MINIATURE_WIDTH))
        .h(px(MINIATURE_HEIGHT))
        .children(rects.iter().map(|(id, rect)| {
            div()
                .absolute()
                .left(px(rect.origin.x))
                .top(px(rect.origin.y))
                .w(px(rect.size.width))
                .h(px(rect.size.height))
                .rounded(px(1.))
                .bg(if *id == pane_id {
                    theme.text
                } else {
                    theme.element_hover
                })
        }))
}

/// A button of the header that shows `lines`.
fn header_button(
    id: &'static str,
    lines: &'static [[(f32, f32); 2]],
    size: f32,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.element_hover;
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(BUTTON_SIZE)
        .rounded_sm()
        .hover(move |style| style.bg(hover))
        // Keep a press on the button from moving the window.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(line_icon(lines, size, theme.text))
}

impl WorkspaceView {
    /// The vertical tabs in `mode`, which shows them.
    pub(super) fn render_vertical_tabs(
        &self,
        mode: VerticalTabs,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let rail = mode == VerticalTabs::Rail;
        let tiling = window_frame::client_tiling(window);
        let round_top_left = tiling.is_some_and(|tiling| !tiling.top && !tiling.left);
        // The bar of errors in the settings takes the bottom when it is shown.
        let round_bottom_left = tiling.is_some_and(|tiling| !tiling.bottom && !tiling.left)
            && self.settings.read(cx).errors().is_empty();
        let rects = self
            .workspace
            .layout()
            .miniature(MINIATURE_WIDTH, MINIATURE_HEIGHT);
        // Truncation is intended; the value is small and positive.
        #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let thumbnail_size = (TILE_SIZE * window.scale_factor()).round() as u32;
        let groups = self.workspace.tab_groups();
        let tab_count = groups.iter().map(|group| group.tabs.len()).sum();
        let list = div()
            .id("vertical-tabs")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.show_app_menu(event.position, window, cx);
                }),
            )
            .children(groups.iter().map(|group| {
                if rail {
                    self.render_rail_group(group, &rects, thumbnail_size, cx)
                } else {
                    self.render_full_group(group, &rects, thumbnail_size, cx)
                }
            }));
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .h_full()
            .w(if rail { RAIL_WIDTH } else { FULL_WIDTH })
            .bg(theme.tab_bar)
            .border_r_1()
            .border_color(theme.border)
            .text_sm()
            .when(round_top_left, |this| this.rounded_tl(ROUNDING))
            .when(round_bottom_left, |this| this.rounded_bl(ROUNDING))
            .child(Self::render_vertical_tabs_header(
                rail, tab_count, window, cx,
            ))
            .child(list)
            .when(rail, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .justify_center()
                        .py_2()
                        .child(Self::new_tab_button(&theme, cx)),
                )
            })
            .into_any_element()
    }

    /// The top of the vertical tabs, at the height of the tab bars. Like
    /// them, it moves the window when the app draws the window frame.
    fn render_vertical_tabs_header(
        rail: bool,
        tab_count: usize,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = theme(cx);
        let (other_mode, switch_text) = if rail {
            (VerticalTabs::Full, "Show the names of the tabs")
        } else {
            (VerticalTabs::Rail, "Show the tabs as a rail")
        };
        let header = div()
            .id("vertical-tabs-header")
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .h(TAB_BAR_HEIGHT)
            .px(px(6.))
            .when(rail, Styled::justify_center)
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.show_app_menu(event.position, window, cx);
                }),
            )
            .child(
                header_button("switch-vertical-tabs", SIDEBAR_LINES, 14., &theme)
                    .tooltip(tooltip(switch_text, None))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.set_vertical_tabs(other_mode, cx);
                    })),
            )
            .when(!rail, |this| {
                this.child(
                    div()
                        .ml_1()
                        .text_xs()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(theme.text_muted)
                        .child("Tabs"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(tab_count.to_string()),
                )
                .child(div().flex_1())
                .child(Self::new_tab_button(&theme, cx))
            });
        if window_frame::client_tiling(window).is_some() {
            Self::moves_window(header, window, cx).into_any_element()
        } else {
            header.into_any_element()
        }
    }

    fn new_tab_button(theme: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        header_button("new-tab", PLUS_LINES, 12., theme)
            .tooltip(tooltip("New tab", None))
            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                cx.stop_propagation();
                this.new_tab(&NewTab, window, cx);
            }))
    }

    /// Makes `element` stand for the pane of `group`: clicking it focuses the
    /// pane, and a tab or image dropped on it goes to the end of the pane.
    fn pane_behavior(
        element: gpui::Stateful<gpui::Div>,
        group: &TabGroup,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let pane_id = group.pane_id;
        let end = group.tabs.len();
        let drop_target = theme(cx).drop_target;
        element
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.focus_pane(pane_id, cx);
            }))
            .drag_over::<DraggedTab>(move |style, _, _, _| style.bg(drop_target))
            .drag_over::<DraggedImage>(move |style, _, _, _| style.bg(drop_target))
            .on_drop(cx.listener(move |this, dragged: &DraggedTab, _, cx| {
                this.move_tab(dragged, pane_id, end, cx);
            }))
            .on_drop(
                cx.listener(move |this, dragged: &DraggedImage, window, cx| {
                    this.open_image_at(dragged, pane_id, end, window, cx);
                }),
            )
    }

    /// A pane in the rail: the miniature of the layout, then a tile for each
    /// tab.
    fn render_rail_group(
        &self,
        group: &TabGroup,
        rects: &[(PaneId, Rect)],
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let pane_id = group.pane_id;
        let hover = theme.element_hover;
        let header = div()
            .id(ElementId::Name(format!("rail-pane-{pane_id:?}").into()))
            .p_1()
            .rounded_sm()
            .hover(move |style| style.bg(hover))
            .child(miniature(rects, pane_id, &theme))
            .when_some(group.name.clone(), |this, name| {
                this.tooltip(tooltip(name, None))
            });
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .py_2()
            .border_t_1()
            .border_color(theme.border)
            .child(Self::pane_behavior(header, group, cx))
            .children(
                group
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(index, tab)| self.render_tile(group, index, tab, thumbnail_size, cx)),
            )
            .into_any_element()
    }

    /// A tab in the rail: its thumbnail, or an icon. A ring marks the active
    /// tab of the pane, in the accent when keys act on it.
    fn render_tile(
        &self,
        group: &TabGroup,
        index: usize,
        tab: &Tab,
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let tab_id = tab.id();
        let label = SharedString::from(tab_name(tab));
        let active = group.active == Some(tab_id);
        let ring = match (active, group.marked) {
            (true, true) => theme.accent,
            (true, false) => theme.text_muted,
            (false, _) => gpui::transparent_black().into(),
        };
        let hover = theme.element_hover;
        let outer = TILE_SIZE + 2. * (TILE_RING_WIDTH + 1.);
        let tile = div()
            .id(ElementId::Name(format!("rail-tab-{tab_id:?}").into()))
            .flex()
            .flex_shrink_0()
            .items_center()
            .justify_center()
            .size(px(outer))
            .rounded(px(7.))
            .border(px(TILE_RING_WIDTH))
            .border_color(ring)
            .when(!active, |this| {
                this.hover(move |style| style.border_color(hover))
            })
            .tooltip(tooltip(label.clone(), group.name.clone().map(Into::into)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(TILE_SIZE))
                    .rounded(px(5.))
                    .bg(theme.element)
                    .child(self.tab_icon(tab, TILE_SIZE, 5., thumbnail_size, cx)),
            );
        Self::tab_behavior(tile, group.pane_id, index, tab_id, label, cx).into_any_element()
    }

    /// A pane in full: a header with the miniature of the layout, the name of
    /// the pane and the number of its tabs, then a row for each tab unless
    /// the pane is collapsed.
    fn render_full_group(
        &self,
        group: &TabGroup,
        rects: &[(PaneId, Rect)],
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let pane_id = group.pane_id;
        let collapsed = self.collapsed_panes.contains(&pane_id);
        let focused = self.workspace.focused_pane() == pane_id;
        let hover = theme.background;
        let header = div()
            .id(ElementId::Name(format!("full-pane-{pane_id:?}").into()))
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1p5()
            .h(GROUP_HEADER_HEIGHT)
            .mx(px(6.))
            .pr_1()
            .rounded_sm()
            .text_xs()
            .hover(move |style| style.bg(hover))
            .child(
                div()
                    .id("collapse")
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .size(px(18.))
                    .rounded_sm()
                    .hover(move |style| style.bg(theme.element_hover))
                    .child(line_icon(
                        if collapsed {
                            COLLAPSED_LINES
                        } else {
                            EXPANDED_LINES
                        },
                        10.,
                        theme.text_muted,
                    ))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        if !this.collapsed_panes.remove(&pane_id) {
                            this.collapsed_panes.insert(pane_id);
                        }
                        cx.notify();
                    })),
            )
            .child(miniature(rects, pane_id, &theme))
            .children(group.name.clone().map(|name| {
                div()
                    .min_w_0()
                    .truncate()
                    .text_color(if focused {
                        theme.text
                    } else {
                        theme.text_muted
                    })
                    .child(name)
            }))
            .when(group.destination, |this| {
                this.child(
                    div()
                        .id("destination")
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .size(px(18.))
                        .rounded_sm()
                        .hover(move |style| style.bg(theme.element_hover))
                        .tooltip(tooltip("New tabs open here", None))
                        .child(line_icon(ANCHOR_LINES, 12., theme.accent))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            cx.stop_propagation();
                            this.toggle_destination_of(pane_id, cx);
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.text_muted)
                    .child(group.tabs.len().to_string()),
            );
        div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_px()
            .pt_1()
            .pb_2()
            .child(Self::pane_behavior(header, group, cx))
            .when(!collapsed, |this| {
                this.children(
                    group
                        .tabs
                        .iter()
                        .enumerate()
                        .map(|(index, tab)| self.render_row(group, index, tab, thumbnail_size, cx)),
                )
            })
            .into_any_element()
    }

    /// A tab in full: its thumbnail or icon, and its name. The active tab of
    /// the pane is filled, and marked on the left when keys act on it. The
    /// close button shows on the active tab and under the mouse.
    fn render_row(
        &self,
        group: &TabGroup,
        index: usize,
        tab: &Tab,
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let pane_id = group.pane_id;
        let tab_id = tab.id();
        let label = SharedString::from(tab_name(tab));
        let active = group.active == Some(tab_id);
        let row_group = SharedString::from(format!("full-tab-{tab_id:?}"));
        let row = div()
            .id(ElementId::Name(row_group.clone()))
            .group(row_group.clone())
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .h(ROW_HEIGHT)
            .mx(px(6.))
            .pl(ROW_INDENT)
            .pr_1()
            .rounded_sm()
            .when(active, |this| {
                this.bg(theme.element).text_color(theme.text_strong)
            })
            .when(!active, |this| {
                this.text_color(theme.text_muted)
                    .hover(move |style| style.bg(theme.background).text_color(theme.text))
            })
            .when(active && group.marked, |this| {
                this.child(
                    div()
                        .absolute()
                        .left_0()
                        .top(px(5.))
                        .bottom(px(5.))
                        .w(px(2.))
                        .rounded(px(1.))
                        .bg(theme.accent),
                )
            })
            .child(self.tab_icon(tab, ROW_ICON_SIZE, 2., thumbnail_size, cx))
            .child(div().flex_1().min_w_0().truncate().child(label.clone()))
            .child(
                div()
                    .id("close")
                    .flex_shrink_0()
                    .px_1()
                    .rounded_sm()
                    .text_color(theme.text_muted)
                    .hover(move |style| style.bg(theme.element_hover))
                    .when(!active, |this| {
                        this.invisible().group_hover(row_group, Styled::visible)
                    })
                    .child("×")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.stop_propagation();
                        this.close_tab(pane_id, tab_id, cx);
                    })),
            );
        Self::tab_behavior(row, pane_id, index, tab_id, label, cx).into_any_element()
    }

    /// What stands for a tab in a square of `size`: the thumbnail of an image,
    /// with corners of `radius`, or an icon.
    fn tab_icon(
        &self,
        tab: &Tab,
        size: f32,
        radius: f32,
        thumbnail_size: u32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        match tab.content() {
            TabContent::Viewer { path } => {
                let key = ThumbnailKey {
                    path: path.clone(),
                    modified: None,
                    size: thumbnail_size,
                };
                match self
                    .thumbnails
                    .update(cx, |thumbnails, cx| thumbnails.get(&key, cx))
                {
                    Some(image) => img(image)
                        .flex_shrink_0()
                        .size(px(size))
                        .rounded(px(radius))
                        .object_fit(ObjectFit::Cover)
                        .into_any_element(),
                    None => div()
                        .flex_shrink_0()
                        .size(px(size))
                        .rounded(px(radius))
                        .bg(theme.placeholder)
                        .into_any_element(),
                }
            }
            TabContent::Explorer { .. } => div()
                .flex()
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .size(px(size))
                .child(folder_icon(size * 0.8, theme.folder))
                .into_any_element(),
            TabContent::Settings => {
                line_icon(SETTINGS_LINES, ROW_ICON_SIZE, theme.text_muted).into_any_element()
            }
        }
    }
}
