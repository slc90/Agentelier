use bevy::prelude::Color;
use bevy_widgetry::theme::{WIDGETRY_DARK_THEME, WidgetryButtonStateColors, WidgetryTheme};

const BACKGROUND: Color = Color::srgb_u8(0x07, 0x1A, 0x12);
const SURFACE: Color = Color::srgb_u8(0x14, 0x35, 0x26);
const RAISED: Color = Color::srgb_u8(0x24, 0x4C, 0x36);
const BORDER: Color = Color::srgb_u8(0x2B, 0x50, 0x3B);
const TEXT: Color = Color::srgb_u8(0xF3, 0xF8, 0xF4);
const MUTED: Color = Color::srgb_u8(0xA6, 0xC5, 0xB0);
const PRIMARY: Color = Color::srgb_u8(0x17, 0x6D, 0x49);
const PRIMARY_HOVER: Color = Color::srgb_u8(0x1D, 0x80, 0x54);
const PRIMARY_PRESS: Color = Color::srgb_u8(0x12, 0x5A, 0x3C);
const FOCUS: Color = Color::srgb_u8(0x67, 0xD3, 0x9A);

pub const AGENTELIER_THEME: WidgetryTheme = {
    let mut theme = WIDGETRY_DARK_THEME;
    theme.button.normal = button(PRIMARY, BORDER, TEXT);
    theme.button.hovered = button(PRIMARY_HOVER, PRIMARY_HOVER, TEXT);
    theme.button.pressed = button(PRIMARY_PRESS, PRIMARY_PRESS, TEXT);
    theme.button.disabled = button(BACKGROUND, BORDER, MUTED);

    theme.window.frame.normal.background = BACKGROUND;
    theme.window.frame.normal.border = BORDER;
    theme.window.frame.normal.foreground = TEXT;
    theme.window.frame.disabled.background = BACKGROUND;
    theme.window.title_bar.normal.foreground = TEXT;
    theme.window.title_bar.normal.border = BORDER;

    theme.text_field.editable.normal.background = SURFACE;
    theme.text_field.editable.hovered.background = RAISED;
    theme.text_field.editable.focused.border = FOCUS;
    theme.file_dialog.accept_button = theme.button;
    theme.message_box.action_button = theme.button;
    theme
};

const fn button(background: Color, border: Color, foreground: Color) -> WidgetryButtonStateColors {
    WidgetryButtonStateColors {
        background,
        border,
        foreground,
    }
}
