//! The colors of the application. All colors come from here.

use deltaflens_core::ThemeChoice;
use gpui::{App, Global, Hsla, Rgba, WindowAppearance, rgb, rgba};

/// The colors of the application, kept as a global and read with
/// [`theme`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    /// Behind everything: the window, the active tab and images.
    pub background: Rgba,
    /// The tab bars, inactive tabs and text fields.
    pub tab_bar: Rgba,
    /// Bars in a tab, such as the toolbar and the path bar of the explorer.
    pub toolbar: Rgba,
    /// Buttons, and things under the mouse in lists.
    pub element: Rgba,
    /// Buttons under the mouse, and the selected button of a toolbar.
    pub element_hover: Rgba,
    /// Menus.
    pub menu: Rgba,
    pub menu_border: Rgba,
    /// The lines between panes and around tabs.
    pub border: Rgba,
    /// The border of a text field without focus.
    pub input_border: Rgba,
    pub text: Rgba,
    /// Text that stands out, such as the active tab.
    pub text_strong: Rgba,
    /// Text of less importance, such as inactive tabs.
    pub text_muted: Rgba,
    /// Messages in place of content, such as an empty folder.
    pub text_placeholder: Rgba,
    pub text_disabled: Rgba,
    /// The focused pane, text fields with focus, and resize handles.
    pub accent: Rgba,
    /// Selected text.
    pub selection: Rgba,
    /// A tab bar that something dragged over it would go to.
    pub drop_target: Rgba,
    /// The part of a pane that something dragged over it would go to.
    pub drop_zone: Rgba,
    pub error_background: Rgba,
    pub error_text: Rgba,
    /// The folders of the explorer.
    pub folder: Rgba,
    /// Stands in for a thumbnail until it is ready.
    pub placeholder: Rgba,
    pub placeholder_border: Rgba,
    /// The close button of the window under the mouse, and its icon.
    pub close_hover: Rgba,
    pub close_icon: Rgba,
    /// The shadow around the window.
    pub shadow: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn dark() -> Self {
        Self {
            background: rgb(0x1e_1e_1e),
            tab_bar: rgb(0x14_14_14),
            toolbar: rgb(0x18_18_18),
            element: rgb(0x2a_2a_2a),
            element_hover: rgb(0x3a_3a_3a),
            menu: rgb(0x24_24_24),
            menu_border: rgb(0x3a_3a_3a),
            border: rgb(0x2a_2a_2a),
            input_border: rgb(0x44_44_44),
            text: rgb(0xd0_d0_d0),
            text_strong: rgb(0xe0_e0_e0),
            text_muted: rgb(0x90_90_90),
            text_placeholder: rgb(0x80_80_80),
            text_disabled: rgb(0x50_50_50),
            accent: rgb(0x5b_9b_e0),
            selection: rgba(0x5b_9b_e0_60),
            drop_target: rgb(0x2a_3a_4a),
            drop_zone: rgba(0x5b_9b_e0_40),
            error_background: rgb(0x4a_22_22),
            error_text: rgb(0xf0_c0_c0),
            folder: rgb(0xc8_a0_4a),
            placeholder: rgb(0x26_26_26),
            placeholder_border: rgb(0x50_50_50),
            close_hover: rgb(0xc4_2b_1c),
            close_icon: rgb(0xf0_f0_f0),
            shadow: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.4,
            },
        }
    }

    pub fn light() -> Self {
        Self {
            background: rgb(0xfa_fa_fa),
            tab_bar: rgb(0xe8_e8_e8),
            toolbar: rgb(0xf0_f0_f0),
            element: rgb(0xe2_e2_e2),
            element_hover: rgb(0xd2_d2_d2),
            menu: rgb(0xff_ff_ff),
            menu_border: rgb(0xc8_c8_c8),
            border: rgb(0xd4_d4_d4),
            input_border: rgb(0xb8_b8_b8),
            text: rgb(0x24_24_24),
            text_strong: rgb(0x00_00_00),
            text_muted: rgb(0x60_60_60),
            text_placeholder: rgb(0x80_80_80),
            text_disabled: rgb(0xb0_b0_b0),
            accent: rgb(0x2f_6f_bf),
            selection: rgba(0x2f_6f_bf_50),
            drop_target: rgb(0xcf_e0_f5),
            drop_zone: rgba(0x2f_6f_bf_40),
            error_background: rgb(0xf8_d7_d7),
            error_text: rgb(0x8a_1f_1f),
            folder: rgb(0xd8_a9_3a),
            placeholder: rgb(0xec_ec_ec),
            placeholder_border: rgb(0xc0_c0_c0),
            close_hover: rgb(0xc4_2b_1c),
            close_icon: rgb(0xff_ff_ff),
            shadow: Hsla {
                h: 0.,
                s: 0.,
                l: 0.,
                a: 0.25,
            },
        }
    }

    /// The theme `choice` asks for, where the system is `appearance`.
    pub fn for_choice(choice: ThemeChoice, appearance: WindowAppearance) -> Self {
        let dark = match choice {
            ThemeChoice::Dark => true,
            ThemeChoice::Light => false,
            ThemeChoice::System => matches!(
                appearance,
                WindowAppearance::Dark | WindowAppearance::VibrantDark
            ),
        };
        if dark { Self::dark() } else { Self::light() }
    }
}

/// The current theme.
pub fn theme(cx: &App) -> Theme {
    *cx.global::<Theme>()
}
