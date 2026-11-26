use agentelier_agent_host::{
    MountedScene, SceneSaveError, SceneTarget, mount_native_scene, patch_native_scene,
    restore_scene_definition,
};
use agentelier_conversation_bridge::{PublishedScene, SessionId};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText};

#[derive(Component, Clone, Default)]
struct Presentation {
    attempted: Option<u64>,
    installed: Option<Mounted>,
    error: Option<Entity>,
}

#[derive(Clone, Copy)]
struct Mounted {
    container: Entity,
    root: Entity,
}

#[derive(Component, Clone, Default)]
struct SaveFeedback;

pub(super) fn card(id: SessionId) -> impl Scene {
    bsn! {
        Name(format!("ChatNativeScene-{id}")) Presentation
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
        Children [
            SaveFeedback Name("ChatSceneSaveFeedback") Text::default() WidgetryText
            TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
            Node { display: Display::None, width: percent(100), min_width: px(0) }
        ]
    }
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        PostUpdate,
        sync_save_feedback.before(bevy::ui::UiSystems::Content),
    );
}

fn sync_save_feedback(
    mut feedbacks: Query<(&ChildOf, &mut Text, &mut Node), With<SaveFeedback>>,
    presentations: Query<&Presentation>,
    errors: Query<&SceneSaveError>,
) {
    for (parent, mut text, node) in &mut feedbacks {
        let feedback = presentations
            .get(parent.parent())
            .ok()
            .and_then(|presentation| presentation.installed)
            .and_then(|installed| errors.get(installed.root).ok())
            .map(|error| format!("控件状态未保存：{}", error.0));
        let display = if feedback.is_some() {
            Display::Flex
        } else {
            Display::None
        };
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(display);
        text.set_if_neq(Text(feedback.unwrap_or_default()));
    }
}

pub(super) fn needs_revision(world: &World, parent: Entity, revision: u64) -> bool {
    world
        .get::<Presentation>(parent)
        .is_some_and(|presentation| {
            presentation
                .attempted
                .is_none_or(|attempted| revision > attempted)
        })
}

pub(super) fn reconcile(
    world: &mut World,
    parent: Entity,
    session: SessionId,
    scene: PublishedScene,
) -> Result {
    let Some(mut presentation) = world.get::<Presentation>(parent).cloned() else {
        return Ok(());
    };
    if presentation
        .attempted
        .is_some_and(|attempted| attempted >= scene.revision)
    {
        return Ok(());
    }
    presentation.attempted = Some(scene.revision);
    if let Some(error) = presentation.error.take() {
        world.despawn(error);
    }
    let target = SceneTarget {
        session,
        artifact: scene.id,
    };
    match apply(world, parent, target, &scene, presentation.installed) {
        Ok(installed) => presentation.installed = Some(installed),
        Err(error) => {
            tracing::error!(artifact = %scene.id, revision = scene.revision, template = %scene.definition.template, %error, "聊天 Scene 呈现失败");
            let message = if presentation.installed.is_some() {
                format!("无法更新控件：{error}")
            } else {
                format!("无法显示控件：{error}")
            };
            presentation.error = Some(
                spawn_scene(
                    world,
                    bsn! {
                        ChildOf(parent) Text(message) WidgetryText
                        TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
                        Node { width: percent(100), min_width: px(0) }
                    },
                )
                .map_err(|failure| failure.into_bevy_error())?,
            );
        }
    }
    world.get_entity_mut(parent)?.insert(presentation);
    Ok(())
}

fn apply(
    world: &mut World,
    parent: Entity,
    target: SceneTarget,
    scene: &PublishedScene,
    installed: Option<Mounted>,
) -> Result<Mounted> {
    let restored = scene
        .state
        .as_ref()
        .map(|state| restore_scene_definition(world, &scene.definition, state))
        .transpose()?;
    let definition = restored.as_ref().unwrap_or(&scene.definition);
    let mount = MountedScene {
        target,
        revision: scene.revision,
    };
    if let Some(installed) = installed
        && patch_native_scene(world, installed.root, definition)?
    {
        world.entity_mut(installed.root).insert(mount);
        return Ok(installed);
    }
    let container = spawn_scene(
        world,
        bsn! {
            ChildOf(parent)
            Node { display: Display::None, width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
        },
    ).map_err(|failure| failure.into_bevy_error())?;
    let root = match mount_native_scene(world, container, definition, mount) {
        Ok(root) => root,
        Err(error) => {
            world.despawn(container);
            return Err(error.into());
        }
    };
    let Some(mut node) = world.get_mut::<Node>(container) else {
        world.despawn(container);
        return Err(std::io::Error::other("原生 Scene 缺少新容器").into());
    };
    node.display = Display::Flex;
    if let Some(old) = installed {
        if let Some(mut focus) = world.get_resource_mut::<InputFocus>()
            && focus.get() == Some(old.root)
        {
            focus.set(root, FocusCause::Navigated);
        }
        world.despawn(old.container);
    }
    Ok(Mounted { container, root })
}
