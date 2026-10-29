//! The menus opened by right-clicking. A window shows at most one, over
//! everything else.

use std::rc::Rc;

use gpui::{
    Action, App, ClickEvent, Context, Entity, KeybindingKeystroke, Modifiers, MouseDownEvent,
    Pixels, Point, SharedString, Window, anchored, deferred, div, prelude::*, px,
};

use crate::theme::{Theme, theme};

/// What an item does when clicked.
type Handler = Rc<dyn Fn(&mut Window, &mut App)>;

/// The items of a menu, and where it opens.
pub struct Menu {
    /// In window coordinates.
    position: Point<Pixels>,
    items: Vec<Item>,
    /// Runs before the action of any item, such as to activate the tab that
    /// was right-clicked.
    before_actions: Option<Handler>,
}

enum Item {
    Entry(Entry),
    Separator,
}

struct Entry {
    label: SharedString,
    /// The key of the action, if it has one.
    shortcut: Option<SharedString>,
    enabled: bool,
    command: Command,
}

enum Command {
    Run(Handler),
    /// Dispatched to the focused element, as its key does, so that both go
    /// through the same handler.
    Action(Box<dyn Action>),
}

impl Menu {
    pub fn new(position: Point<Pixels>) -> Self {
        Self {
            position,
            items: Vec::new(),
            before_actions: None,
        }
    }

    /// An item that runs `handler`, after the menu is closed.
    pub fn item(
        mut self,
        label: impl Into<SharedString>,
        handler: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.items.push(Item::Entry(Entry {
            label: label.into(),
            shortcut: None,
            enabled: true,
            command: Command::Run(Rc::new(handler)),
        }));
        self
    }

    /// An item that dispatches `action`, showing its key. It is grayed out
    /// and does nothing unless `enabled`.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        action: impl Action,
        enabled: bool,
        window: &Window,
    ) -> Self {
        let shortcut = window
            .highest_precedence_binding_for_action(&action)
            .map(|binding| shortcut_text(binding.keystrokes()).into());
        self.items.push(Item::Entry(Entry {
            label: label.into(),
            shortcut,
            enabled,
            command: Command::Action(Box::new(action)),
        }));
        self
    }

    /// A line between groups of items.
    pub fn separator(mut self) -> Self {
        self.items.push(Item::Separator);
        self
    }

    /// Runs `prepare` before the action of any item.
    pub fn before_actions(mut self, prepare: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.before_actions = Some(Rc::new(prepare));
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

fn render_entry(
    index: usize,
    entry: &Entry,
    before_actions: Option<&Handler>,
    context_menu: Entity<ContextMenu>,
    theme: &Theme,
) -> impl IntoElement + use<> {
    let (label_color, shortcut_color) = if entry.enabled {
        (theme.text, theme.text_muted)
    } else {
        (theme.text_disabled, theme.text_disabled)
    };
    let handler: Handler = match &entry.command {
        Command::Run(handler) => handler.clone(),
        Command::Action(action) => {
            let action = action.boxed_clone();
            let before = before_actions.cloned();
            Rc::new(move |window, cx| {
                if let Some(before) = &before {
                    before(window, cx);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            })
        }
    };
    let hover = theme.element_hover;
    div()
        .id(("menu-item", index))
        .flex()
        .justify_between()
        .gap_6()
        .px_3()
        .py_1()
        .rounded_sm()
        .text_color(label_color)
        .child(entry.label.clone())
        .children(
            entry
                .shortcut
                .clone()
                .map(|shortcut| div().text_color(shortcut_color).child(shortcut)),
        )
        .when(entry.enabled, |this| {
            this.hover(move |style| style.bg(hover))
                // Not a listener of the menu, so that the handler may update
                // it.
                .on_click(move |_: &ClickEvent, window, cx| {
                    context_menu.update(cx, ContextMenu::dismiss);
                    handler(window, cx);
                })
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
            .map(|(index, item)| match item {
                Item::Entry(entry) => render_entry(
                    index,
                    entry,
                    menu.before_actions.as_ref(),
                    entity.clone(),
                    &theme,
                )
                .into_any_element(),
                Item::Separator => div()
                    .h(px(1.))
                    .my_1()
                    .bg(theme.menu_border)
                    .into_any_element(),
            })
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

/// How a key binding is shown in a menu, such as "Ctrl+K →".
fn shortcut_text(keystrokes: &[KeybindingKeystroke]) -> String {
    keystrokes
        .iter()
        .map(|keystroke| {
            // No key of the application uses the other modifiers, whose names
            // depend on the system, so gpui names them.
            let modifiers = keystroke.modifiers();
            if modifiers.platform || modifiers.function {
                keystroke.to_string()
            } else {
                keystroke_text(*modifiers, keystroke.key())
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn keystroke_text(modifiers: Modifiers, key: &str) -> String {
    let mut text = String::new();
    for (pressed, name) in [
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
    ] {
        if pressed {
            text.push_str(name);
            text.push('+');
        }
    }
    match key {
        "left" => text.push('←'),
        "right" => text.push('→'),
        "up" => text.push('↑'),
        "down" => text.push('↓'),
        "escape" => text.push_str("Esc"),
        _ => {
            let mut chars = key.chars();
            if let Some(first) = chars.next() {
                text.extend(first.to_uppercase());
                text.push_str(chars.as_str());
            }
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use gpui::Keystroke;

    use super::*;

    fn shortcut(source: &str) -> String {
        let keystrokes: Vec<_> = source
            .split(' ')
            .map(|keystroke| {
                KeybindingKeystroke::from_keystroke(Keystroke::parse(keystroke).unwrap())
            })
            .collect();
        shortcut_text(&keystrokes)
    }

    #[test]
    fn letters_are_upper_case_after_the_modifiers() {
        assert_eq!(shortcut("ctrl-shift-t"), "Ctrl+Shift+T");
    }

    #[test]
    fn arrows_are_shown_as_arrows() {
        assert_eq!(shortcut("ctrl-alt-shift-left"), "Ctrl+Alt+Shift+←");
        assert_eq!(shortcut("right"), "→");
    }

    #[test]
    fn keys_pressed_one_after_another_are_separated_by_spaces() {
        assert_eq!(shortcut("ctrl-k down"), "Ctrl+K ↓");
        assert_eq!(shortcut("ctrl-k 1"), "Ctrl+K 1");
    }

    #[test]
    fn punctuation_and_named_keys() {
        assert_eq!(shortcut("ctrl-,"), "Ctrl+,");
        assert_eq!(shortcut("ctrl-tab"), "Ctrl+Tab");
        assert_eq!(shortcut("escape"), "Esc");
    }
}
