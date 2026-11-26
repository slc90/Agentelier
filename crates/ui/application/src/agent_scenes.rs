use agentelier_agent_host::AgentHostPlugin;
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::prelude::*;
use bevy_widgetry::text::WidgetryText;
use serde::Deserialize;

const MAX_LABEL_BYTES: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ButtonProperties {
    label: String,
    style: ButtonStyle,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
    Icon,
}

#[derive(Component, Clone, Default)]
struct NativeButtonLabel;

pub(crate) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene(
        "agentelier.button",
        "Create an Agentelier button with a Widgetry text label under a live UI Node.",
        &[
            (
                "label",
                "Required nonblank string, at most 256 UTF-8 bytes.",
            ),
            ("style", "Required: primary, secondary, danger, or icon."),
        ],
        button,
        Some(update_button),
    )
}

fn button(properties: ButtonProperties) -> Result<impl Scene> {
    let (label, style) = button_fields(properties)?;
    let children: Box<dyn SceneList> =
        Box::new(bsn_list! { Text(label) WidgetryText NativeButtonLabel });
    Ok(bsn! {
        @AgentelierButton { @style: style, @children: {Some(children)} }
        Node { align_self: AlignSelf::Start }
    })
}

fn update_button(world: &mut World, root: Entity, properties: ButtonProperties) -> Result {
    let (label, style) = button_fields(properties)?;
    let mut pending = vec![root];
    let mut text = None;
    while let Some(entity) = pending.pop() {
        if world.get::<NativeButtonLabel>(entity).is_some() {
            text = Some(entity);
            break;
        }
        if let Some(children) = world.get::<Children>(entity) {
            pending.extend(children.iter());
        }
    }
    let Some(text) = text.filter(|entity| world.get::<Text>(*entity).is_some()) else {
        return Err(std::io::Error::other("原生按钮缺少标签").into());
    };
    let Some(mut button) = world.get_mut::<AgentelierButton>(root) else {
        return Err(std::io::Error::other("原生按钮实体无效").into());
    };
    button.style = style;
    world
        .get_mut::<Text>(text)
        .ok_or_else(|| std::io::Error::other("原生按钮缺少标签"))?
        .set_if_neq(Text(label));
    Ok(())
}

fn button_fields(properties: ButtonProperties) -> Result<(String, AgentelierButtonStyle)> {
    let ButtonProperties { label, style } = properties;
    if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(std::io::Error::other("按钮标签必须非空且长度不超过 256 字节").into());
    }
    let style = match style {
        ButtonStyle::Primary => AgentelierButtonStyle::Primary,
        ButtonStyle::Secondary => AgentelierButtonStyle::Secondary,
        ButtonStyle::Danger => AgentelierButtonStyle::Danger,
        ButtonStyle::Icon => AgentelierButtonStyle::Icon,
    };
    Ok((label, style))
}
