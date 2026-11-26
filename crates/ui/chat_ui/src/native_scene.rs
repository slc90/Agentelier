use agentelier_agent_host::mount_native_scene;
use agentelier_conversation_bridge::{PublishedScene, SessionId};
use bevy::prelude::*;
use bevy_widgetry::{scene::WidgetrySceneCommandsExt, text::WidgetryText};

pub(super) fn card(id: SessionId) -> impl Scene {
    bsn! {
        Name(format!("ChatNativeScene-{id}"))
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
    }
}

pub(super) fn mount(world: &mut World, parent: Entity, scene: PublishedScene) {
    if !world.entities().contains_spawned(parent) {
        return;
    }
    let Err(error) = mount_native_scene(world, parent, &scene.definition) else {
        return;
    };
    tracing::error!(artifact = %scene.id, template = %scene.definition.template, %error, "聊天 Scene 构造失败");
    if let Some(children) = world.get::<Children>(parent) {
        let owned: Vec<_> = children.iter().collect();
        for child in owned {
            world.despawn(child);
        }
    }
    let message = format!("无法显示控件：{error}");
    world.commands().spawn_scene_with_error_handler(bsn! {
        ChildOf(parent) Text(message) WidgetryText
        TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        Node { width: percent(100), min_width: px(0) }
    });
}
