use crate::AGENTELIER_THEME;
use bevy::input_focus::{InputFocus, tab_navigation::TabIndex};
use bevy::picking::{cursor::EntityCursor, hover::Hovered};
use bevy::prelude::*;
use bevy::ui::{Checked, Pressed, UiSystems};
use bevy::window::SystemCursorIcon;
use bevy_widgetry::disabled::WidgetryEffectiveDisabled;
use bevy_widgetry::toggle_button::{
    WidgetryToggleButtonHeadless, WidgetryToggleButtonHeadlessPlugin,
};

const TRACK_WIDTH: f32 = 38.0;
const TRACK_HEIGHT: f32 = 22.0;
const TRACK_PADDING: f32 = 3.0;
const THUMB_SIZE: f32 = TRACK_HEIGHT - TRACK_PADDING * 2.0;
const FOCUS_WIDTH: f32 = 2.0;
const OFF_TRACK: Color = Color::srgb_u8(143, 166, 153);
const OFF_HOVERED: Color = Color::srgb_u8(120, 150, 134);
const THUMB: Color = Color::srgb_u8(247, 252, 248);

#[derive(Component, Clone, Default)]
struct SwitchTrack;

pub fn agentelier_switch() -> impl Scene {
    bsn! {
        @WidgetryToggleButtonHeadless {}
        SwitchTrack TabIndex(0)
        template(|_| Ok(EntityCursor::System(SystemCursorIcon::Pointer)))
        template(|_| Ok(Outline::new(px(FOCUS_WIDTH), px(FOCUS_WIDTH), Color::NONE)))
        BackgroundColor(OFF_TRACK)
        Node {
            width: px(TRACK_WIDTH), height: px(TRACK_HEIGHT), flex_shrink: 0.0,
            border_radius: BorderRadius::MAX, padding: UiRect::all(px(TRACK_PADDING)),
            align_items: AlignItems::Center,
        }
        Children [
            Pickable::IGNORE BackgroundColor(THUMB)
            Node { width: px(THUMB_SIZE), height: px(THUMB_SIZE), border_radius: BorderRadius::MAX }
        ]
    }
}

pub(super) fn install(app: &mut App) {
    if !app.is_plugin_added::<WidgetryToggleButtonHeadlessPlugin>() {
        app.add_plugins(WidgetryToggleButtonHeadlessPlugin);
    }
    app.add_systems(
        PostUpdate,
        sync.after(UiSystems::Content).before(UiSystems::Layout),
    );
}

fn sync(
    focus: Res<InputFocus>,
    mut switches: Query<
        (
            Entity,
            Has<Checked>,
            Has<Pressed>,
            &Hovered,
            &WidgetryEffectiveDisabled,
            &mut Node,
            &mut BackgroundColor,
            &mut Outline,
        ),
        With<SwitchTrack>,
    >,
) {
    let theme = &AGENTELIER_THEME.button;
    for (entity, checked, pressed, hovered, disabled, node, mut background, outline) in
        &mut switches
    {
        let disabled = disabled.is_disabled();
        let color = match (checked, disabled, pressed, hovered.0) {
            (_, true, _, _) => theme.disabled.border,
            (true, false, true, _) => theme.pressed.border,
            (true, false, false, true) => theme.hovered.border,
            (true, false, false, false) => theme.normal.border,
            (false, false, _, true) => OFF_HOVERED,
            (false, false, _, false) => OFF_TRACK,
        };
        let align = if checked {
            JustifyContent::FlexEnd
        } else {
            JustifyContent::FlexStart
        };
        node.map_unchanged(|node| &mut node.justify_content)
            .set_if_neq(align);
        background.set_if_neq(BackgroundColor(color));
        let focus = if !disabled && focus.get() == Some(entity) {
            theme.normal.border
        } else {
            Color::NONE
        };
        outline
            .map_unchanged(|outline| &mut outline.color)
            .set_if_neq(focus);
    }
}
