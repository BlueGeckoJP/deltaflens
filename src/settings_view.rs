use deltaflens_core::{IMAGE_CACHE_MIBS, Settings, THUMBNAIL_SIZES};
use gpui::{AnyElement, Context, Entity, SharedString, Window, div, prelude::*, px, rgb};

use crate::{
    settings::SettingsStore,
    text_input::{Submitted, TextInput},
};

/// How much one press of a button changes the size of thumbnails.
const THUMBNAIL_SIZE_STEP: u32 = 32;

/// The settings tab: shows the settings and changes them.
pub struct SettingsView {
    settings: Entity<SettingsStore>,
    open_command: Entity<TextInput>,
}

impl SettingsView {
    pub fn new(
        settings: Entity<SettingsStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let command: SharedString = settings.read(cx).settings().open_command.clone().into();
        let open_command = cx.new(|cx| TextInput::new(command, window, cx));
        cx.subscribe(&open_command, |this, _, Submitted(text), cx| {
            let text = text.trim().to_owned();
            if text.is_empty() {
                return;
            }
            this.settings.update(cx, |settings, cx| {
                settings.update(|s| s.open_command = text, cx);
            });
        })
        .detach();
        cx.observe_in(&settings, window, |this, settings, window, cx| {
            let command = settings.read(cx).settings().open_command.clone();
            this.open_command
                .update(cx, |input, cx| input.set_text(command.into(), window, cx));
            cx.notify();
        })
        .detach();
        Self {
            settings,
            open_command,
        }
    }

    fn change(&self, change: impl FnOnce(&mut Settings), cx: &mut Context<Self>) {
        self.settings
            .update(cx, |settings, cx| settings.update(change, cx));
    }

    /// The size of thumbnails, changed in steps of [`THUMBNAIL_SIZE_STEP`].
    fn render_thumbnail_size(size: u32, cx: &mut Context<Self>) -> impl IntoElement {
        let (min, max) = (*THUMBNAIL_SIZES.start(), *THUMBNAIL_SIZES.end());
        let smaller = size.saturating_sub(THUMBNAIL_SIZE_STEP).max(min);
        let larger = (size + THUMBNAIL_SIZE_STEP).min(max);
        row(
            "Thumbnail size",
            stepper(
                "thumbnail-size",
                format!("{size} px"),
                size > min,
                size < max,
                cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.thumbnail_size = smaller, cx);
                }),
                cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.thumbnail_size = larger, cx);
                }),
            ),
        )
    }

    /// The memory for decoded images, halved or doubled.
    fn render_image_cache(mib: u32, cx: &mut Context<Self>) -> impl IntoElement {
        let (min, max) = (*IMAGE_CACHE_MIBS.start(), *IMAGE_CACHE_MIBS.end());
        let smaller = (mib / 2).max(min);
        let larger = mib.saturating_mul(2).min(max);
        row(
            "Memory for images",
            stepper(
                "image-cache",
                format!("{mib} MiB"),
                mib > min,
                mib < max,
                cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.image_cache_mib = smaller, cx);
                }),
                cx.listener(move |this, _, _, cx| {
                    this.change(|s| s.image_cache_mib = larger, cx);
                }),
            ),
        )
    }
}

impl Render for SettingsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let store = self.settings.read(cx);
        let settings = store.settings().clone();
        let errors = store.errors().to_vec();
        let path = Settings::path().map_or_else(
            || "unknown, so settings are not saved".to_owned(),
            |path| path.display().to_string(),
        );

        let thumbnail_size = settings.thumbnail_size;
        let image_cache_mib = settings.image_cache_mib;
        let show_hidden = settings.show_hidden_files;
        div()
            .id("settings")
            .size_full()
            .overflow_y_scroll()
            .p_6()
            .flex()
            .flex_col()
            .gap_4()
            .text_color(rgb(0xd0_d0_d0))
            .child(div().text_xl().child("Settings"))
            .child(
                div()
                    .text_sm()
                    .text_color(rgb(0x90_90_90))
                    .child(format!("Saved in {path}")),
            )
            .children(errors.into_iter().map(|error| {
                div()
                    .px_3()
                    .py_2()
                    .rounded_sm()
                    .bg(rgb(0x4a_22_22))
                    .child(error)
            }))
            .child(Self::render_thumbnail_size(thumbnail_size, cx))
            .child(row(
                "Show hidden files",
                button(
                    "show-hidden",
                    if show_hidden { "On" } else { "Off" },
                    true,
                    cx.listener(move |this, _, _, cx| {
                        this.change(|s| s.show_hidden_files = !show_hidden, cx);
                    }),
                ),
            ))
            .child(Self::render_image_cache(image_cache_mib, cx))
            .child(row(
                "Open with (<path> is the file; Enter to apply)",
                div().w(px(320.)).child(self.open_command.clone()),
            ))
    }
}

/// A setting: its name on the left, its control on the right.
fn row(label: &'static str, control: impl IntoElement) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_4()
        .max_w(px(640.))
        .child(label)
        .child(control)
}

type ClickHandler = Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App)>;

/// A value with buttons that make it smaller and larger.
fn stepper(
    id: &'static str,
    value: String,
    can_decrease: bool,
    can_increase: bool,
    decrease: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
    increase: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let decrease: ClickHandler = Box::new(decrease);
    let increase: ClickHandler = Box::new(increase);
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .child(button(
            SharedString::from(format!("{id}-decrease")),
            "−",
            can_decrease,
            decrease,
        ))
        .child(div().w(px(96.)).flex().justify_center().child(value))
        .child(button(
            SharedString::from(format!("{id}-increase")),
            "+",
            can_increase,
            increase,
        ))
        .into_any_element()
}

fn button(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    enabled: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .px_3()
        .py_1()
        .rounded_sm()
        .bg(rgb(0x2a_2a_2a))
        .when(enabled, |this| {
            this.hover(|style| style.bg(rgb(0x3a_3a_3a)))
                .on_click(on_click)
        })
        .when(!enabled, |this| this.text_color(rgb(0x60_60_60)))
        .child(label)
}
