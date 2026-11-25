use crate::AGENTELIER_THEME;
use bevy::color::Mix;
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy_widgetry::button::{
    WidgetryButton, WidgetryButtonColorOverrides, WidgetryButtonStateColorOverrides,
};
use bevy_widgetry::disabled::WidgetryEffectiveDisabled;
use bevy_widgetry::theme::{WIDGETRY_LIGHT_THEME, WidgetryTheme};

const BUTTON_HEIGHT: f32 = 36.0;
const BUTTON_RADIUS: f32 = 6.0;
const HORIZONTAL_PADDING: f32 = 12.0;
const VERTICAL_PADDING: f32 = 4.0;
const FOCUS_WIDTH: f32 = 2.0;
const DANGER_IDLE_INTENSITY: f32 = 0.22;

#[derive(Clone, Copy, Default, Reflect)]
pub enum AgentelierButtonStyle {
    #[default]
    Primary,
    Secondary,
    Danger,
    FilledDanger,
    Icon,
    DangerIcon,
}

#[derive(SceneComponent, Clone, Default, Reflect)]
#[reflect(Component)]
#[scene(AgentelierButtonProps)]
pub struct AgentelierButton {
    pub style: AgentelierButtonStyle,
}

#[derive(Default)]
pub struct AgentelierButtonProps {
    pub style: AgentelierButtonStyle,
    pub children: Option<Box<dyn SceneList>>,
}

impl AgentelierButton {
    fn scene(props: AgentelierButtonProps) -> impl Scene {
        let AgentelierButtonProps { style, children } = props;
        let mut node = Node::default();
        style.shape(&mut node);
        let padding = node.padding;
        let border_radius = node.border_radius;
        let aspect_ratio = node.aspect_ratio;
        bsn! {
            @WidgetryButton { @children: {children}, @colors: {style.colors(&AGENTELIER_THEME)} }
            AgentelierButton { style: style }
            Node {
                min_height: px(BUTTON_HEIGHT), flex_shrink: 0.0,
                padding: padding, border_radius: border_radius, aspect_ratio: aspect_ratio,
            }
            template(|_| Ok(Outline::new(px(FOCUS_WIDTH), px(-FOCUS_WIDTH), Color::NONE)))
        }
    }
}

impl AgentelierButtonStyle {
    fn shape(self, node: &mut Node) {
        let icon = matches!(self, Self::Icon | Self::DangerIcon);
        node.aspect_ratio = icon.then_some(1.0);
        node.border_radius = BorderRadius::all(if icon { percent(50) } else { px(BUTTON_RADIUS) });
        node.padding = if icon {
            UiRect::ZERO
        } else {
            UiRect::axes(px(HORIZONTAL_PADDING), px(VERTICAL_PADDING))
        };
    }

    fn colors(self, theme: &WidgetryTheme) -> WidgetryButtonColorOverrides {
        let surface = |color, text| WidgetryButtonStateColorOverrides {
            background: Some(color),
            foreground: Some(text),
            ..default()
        };
        match self {
            Self::Primary => default(),
            Self::Secondary | Self::Icon => WidgetryButtonColorOverrides {
                normal: surface(
                    theme.text_field.editable.normal.background,
                    theme.text.normal.foreground,
                ),
                hovered: surface(
                    theme.list_view.item.hovered.background,
                    theme.text.normal.foreground,
                ),
                pressed: surface(
                    theme.list_view.item.selected.background,
                    theme.text.normal.foreground,
                ),
                ..default()
            },
            Self::Danger => {
                let danger = WIDGETRY_LIGHT_THEME.window.close;
                WidgetryButtonColorOverrides {
                    normal: surface(
                        theme
                            .window
                            .frame
                            .normal
                            .background
                            .mix(&Color::NONE, DANGER_IDLE_INTENSITY),
                        theme.text.normal.foreground,
                    ),
                    hovered: surface(Color::NONE, danger.hovered.background),
                    pressed: surface(Color::NONE, danger.pressed.background),
                    ..default()
                }
            }
            Self::FilledDanger => {
                let red = |background, foreground| WidgetryButtonStateColorOverrides {
                    background: Some(background),
                    border: Some(background),
                    foreground: Some(foreground),
                };
                WidgetryButtonColorOverrides {
                    normal: red(Color::srgb_u8(0xAA, 0x30, 0x39), Color::WHITE),
                    hovered: red(Color::srgb_u8(0xBE, 0x36, 0x40), Color::WHITE),
                    pressed: red(Color::srgb_u8(0x91, 0x26, 0x30), Color::WHITE),
                    disabled: red(
                        Color::srgb_u8(0xF4, 0xD3, 0xD4),
                        Color::srgb_u8(0x90, 0x50, 0x55),
                    ),
                }
            }
            Self::DangerIcon => {
                let icon_surface = |background| WidgetryButtonStateColorOverrides {
                    background: Some(background),
                    border: Some(Color::srgb_u8(0xDE, 0x9C, 0xB2)),
                    foreground: Some(Color::srgb_u8(0x9E, 0x37, 0x55)),
                };
                WidgetryButtonColorOverrides {
                    normal: icon_surface(Color::srgb_u8(0xFF, 0xE6, 0xED)),
                    hovered: icon_surface(Color::srgb_u8(0xFF, 0xD7, 0xE2)),
                    pressed: icon_surface(Color::srgb_u8(0xF6, 0xC0, 0xD1)),
                    ..default()
                }
            }
        }
    }
}

pub(crate) fn install(app: &mut App) {
    app.register_type::<AgentelierButton>().add_systems(
        PostUpdate,
        (
            update_style.before(UiSystems::Prepare),
            update_focus.before(UiSystems::Layout),
        ),
    );
}

fn update_style(
    theme: Res<WidgetryTheme>,
    mut buttons: Query<(Entity, Ref<AgentelierButton>, &mut Node)>,
    mut commands: Commands,
) {
    for (entity, button, mut node) in &mut buttons {
        if button.is_changed() {
            button.style.shape(&mut node);
        }
        if button.is_changed() || theme.is_changed() {
            let colors = button.style.colors(&theme);
            commands.queue(move |world: &mut World| -> Result {
                // A virtual row can be replaced before this style update is applied.
                if world.get_entity(entity).is_err() {
                    return Ok(());
                }
                WidgetryButtonColorOverrides::set_in_world(world, entity, colors)
                    .map_err(|failure| failure.into_bevy_error())?;
                Ok(())
            });
        }
    }
}

fn update_focus(
    theme: Res<WidgetryTheme>,
    focus: Res<InputFocus>,
    mut buttons: Query<
        (Entity, Option<&WidgetryEffectiveDisabled>, &mut Outline),
        With<AgentelierButton>,
    >,
) {
    for (entity, disabled, outline) in &mut buttons {
        let color = if focus.get() == Some(entity)
            && !disabled.is_some_and(WidgetryEffectiveDisabled::is_disabled)
        {
            theme.text_field.editable.focused.border
        } else {
            Color::NONE
        };
        outline
            .map_unchanged(|outline| &mut outline.color)
            .set_if_neq(color);
    }
}
