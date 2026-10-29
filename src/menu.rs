//! The menus opened by right-clicking. A window shows at most one, over
//! everything else.

use std::rc::Rc;

use gpui::{
    App, ClickEvent, Context, Entity, MouseDownEvent, Pixels, Point, SharedString, Window,
    anchored, deferred, div, prelude::*, px,
};

use crate::theme::{Theme, theme};

/// What an item does when clicked.
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The items of a menu, and where it opens.
pub struct Menu {
    /// In window coordinates.
    position: Point<Pixels>,
    items: Vec<Item>,
}

struct Item {
    label: SharedString,
    handler: Handler,
}

impl Menu {
    pub fn new(position: Point<Pixels>) -> Self {
        Self {
            position,
            items: Vec::new(),
        }
    }

    /// An item that runs `handler`, after the menu is closed.
    pub fn item(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item {
            label: label.into(),
            handler: Rc::new(handler),
        });
        self
    }
}

impl FluentBuilder for Menu {}

/// The menu open in a window, if any. Opening a menu closes the one open
/// before. It closes by pressing the mouse outside of it, or a key, which the
/// window passes to [`Self::dismiss`].
#[derive(Default)]
pub struct ContextMenu {
    menu: Option<Menu>,
}

impl ContextMenu {
    pub fn show(&mut self, menu: Menu, cx: &mut Context<Self>) {
        self.menu = Some(menu);
        cx.notify();
    }

    /// Closes the menu. Returns whether one was open.
    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.menu.take().is_some();
        if open {
            cx.notify();
        }
        open
    }
}

fn render_item(
    index: usize,
    item: &Item,
    context_menu: Entity<ContextMenu>,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let handler = item.handler.clone();
    let hover = theme.element_hover;
    div()
        .id(("menu-item", index))
        .px_3()
        .py_1()
        .rounded_sm()
        .hover(move |style| style.bg(hover))
        .child(item.label.clone())
        // Not a listener of the menu, so that the handler may update it.
        .on_click(move |_: &ClickEvent, window, cx| {
            context_menu.update(cx, ContextMenu::dismiss);
            handler(window, cx);
        })
}

impl Render for ContextMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(menu) = &self.menu else {
            return div().into_any_element();
        };
        let theme = theme(cx);
        let entity = cx.entity();
        let items: Vec<_> = menu
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| render_item(index, item, entity.clone(), &theme))
            .collect();
        deferred(
            anchored().position(menu.position).snap_to_window().child(
                div()
                    .flex()
                    .flex_col()
                    .min_w(px(180.))
                    .p_1()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.menu_border)
                    .bg(theme.menu)
                    .text_sm()
                    .text_color(theme.text)
                    .shadow_md()
                    // Keep clicks on the menu from reaching the elements under
                    // it.
                    .occlude()
                    .on_mouse_down_out(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        this.dismiss(cx);
                    }))
                    .children(items),
            ),
        )
        .with_priority(1)
        .into_any_element()
    }
}
