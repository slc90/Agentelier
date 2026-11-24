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
    )
}

fn button(properties: ButtonProperties) -> Result<impl Scene> {
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
    let children: Box<dyn SceneList> = Box::new(bsn_list! { Text(label) WidgetryText });
    Ok(bsn! {
        @AgentelierButton { @style: style, @children: {Some(children)} }
        Node { align_self: AlignSelf::Start }
    })
}
