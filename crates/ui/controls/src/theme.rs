use bevy::prelude::Color;
use bevy_widgetry::theme::{WIDGETRY_LIGHT_THEME, WidgetryButtonStateColors, WidgetryTheme};

const TOOLBAR: Color = Color::srgb_u8(0xF4, 0xFB, 0xF6);
const NAVIGATION: Color = Color::srgb_u8(0xED, 0xF6, 0xEE);
const INPUT: Color = Color::srgb_u8(0xF7, 0xFB, 0xF7);
const HOVER: Color = Color::srgb_u8(0xE4, 0xF0, 0xE8);
const SELECTED: Color = Color::srgb_u8(0xD8, 0xEB, 0xDF);
const BORDER: Color = Color::srgb_u8(0xB8, 0xD2, 0xC3);
const TEXT: Color = Color::srgb_u8(0x21, 0x3D, 0x32);
const TITLE: Color = Color::srgb_u8(0x24, 0x48, 0x3A);
const MUTED: Color = Color::srgb_u8(0x45, 0x66, 0x56);
const PRIMARY: Color = Color::srgb_u8(0x23, 0x5E, 0x47);
const PRIMARY_TEXT: Color = Color::srgb_u8(0xEE, 0xF8, 0xEF);
const PRIMARY_HOVER: Color = Color::srgb_u8(0x2C, 0x6C, 0x51);
const PRIMARY_PRESS: Color = Color::srgb_u8(0x19, 0x4A, 0x36);

pub const AGENTELIER_THEME: WidgetryTheme = {
    let mut theme = WIDGETRY_LIGHT_THEME;
    theme.text.normal.foreground = TEXT;
    theme.button.normal = button(PRIMARY, PRIMARY, PRIMARY_TEXT);
    theme.button.hovered = button(PRIMARY_HOVER, PRIMARY_HOVER, PRIMARY_TEXT);
    theme.button.pressed = button(PRIMARY_PRESS, PRIMARY_PRESS, PRIMARY_TEXT);
    theme.button.disabled = button(NAVIGATION, BORDER, MUTED);

    theme.window.frame.normal.background = TOOLBAR;
    theme.window.frame.normal.border = BORDER;
    theme.window.frame.normal.foreground = TEXT;
    theme.window.frame.disabled.background = NAVIGATION;
    theme.window.title_bar.normal.background = TOOLBAR;
    theme.window.title_bar.normal.foreground = TITLE;
    theme.window.title_bar.normal.border = BORDER;
    // Keep the close feedback inside the curved frame at the end of the title bar.
    theme.window.close.hovered.foreground = theme.window.close.hovered.background;
    theme.window.close.pressed.foreground = theme.window.close.pressed.background;
    theme.window.close.hovered.background = Color::NONE;
    theme.window.close.pressed.background = Color::NONE;

    theme.text_field.editable.normal.background = INPUT;
    theme.text_field.editable.normal.border = BORDER;
    theme.text_field.editable.normal.foreground = TEXT;
    theme.text_field.editable.hovered = theme.text_field.editable.normal;
    theme.text_field.editable.hovered.border = PRIMARY_HOVER;
    theme.text_field.editable.focused = theme.text_field.editable.normal;
    theme.text_field.editable.focused.border = PRIMARY;
    theme.text_field.editable.focused.selection_background = SELECTED;

    theme.combo_box.field.normal.background = INPUT;
    theme.combo_box.field.normal.border = BORDER;
    theme.combo_box.field.normal.foreground = TEXT;
    theme.combo_box.field.hovered.background = HOVER;
    theme.combo_box.field.pressed.background = SELECTED;
    theme.combo_box.field.open.background = SELECTED;
    theme.combo_box.field.disabled.foreground = MUTED;
    theme.slider.normal.fill = PRIMARY;
    theme.slider.normal.thumb = PRIMARY;
    theme.slider.hovered.fill = PRIMARY_HOVER;
    theme.slider.hovered.thumb = PRIMARY_HOVER;
    theme.slider.pressed.fill = PRIMARY_PRESS;
    theme.slider.pressed.thumb = PRIMARY_PRESS;
    theme.list_view.item.hovered.background = HOVER;
    theme.list_view.item.selected.background = SELECTED;
    theme.list_view.item.selected.foreground = TEXT;
    theme.tree.container.normal.background = NAVIGATION;
    theme.tree.container.focused = theme.tree.container.normal;
    theme.tree.container.focused.border = PRIMARY;
    theme.tree.item = theme.list_view.item;
    theme.combo_box.list.item = theme.list_view.item;

    theme.file_dialog.window = theme.window;
    // Dialogs fill the window body with a square node. Let the frame paint the corners.
    theme.file_dialog.body.normal.background = Color::NONE;
    theme.file_dialog.body.disabled.background = Color::NONE;
    theme.file_dialog.entry.selected.background = SELECTED;
    theme.file_dialog.entry.selected.foreground = TEXT;
    theme.file_dialog.accept_button = theme.button;
    theme.message_box.window = theme.window;
    theme.message_box.body.normal.background = Color::NONE;
    theme.message_box.body.disabled.background = Color::NONE;
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
