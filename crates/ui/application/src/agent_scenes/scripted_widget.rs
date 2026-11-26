use agentelier_agent_host::{
    AgentHost, AgentHostPlugin, EcsError, SceneDefinition, mount_scene_child, validating_scene,
    widget,
};
use bevy::prelude::*;
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText};
use serde_json::{Map, Value};

#[derive(Component)]
struct ScriptedWidget {
    status: Entity,
    load: Load,
}

enum Load {
    Queued(Box<widget::Definition>),
    Running(widget::Ticket),
    Done,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        widget::TEMPLATE,
        "Publish a native, independently restorable widget authored in Rhai. The program constructs stable native child scenes and computes its initial state.",
        &[
            ("source", "Self-contained Rhai. Define fn init(ctx) returning #{parts: [...], state: #{...}}. ctx.parameters and ctx.resources are maps. Optional fn restore(ctx) receives the saved initial ctx.state and returns the same shape, otherwise init(ctx) is used. Use publish_scene for validation."),
            ("parameters", "Optional serializable map furnished to both callbacks."),
            ("resources", "Optional serializable resource snapshots furnished to both callbacks."),
            ("parts returned by source", "1 to 16 native composition parts, each #{id, area, scene: #{template, props}}. area is intro, controls, content, or footer. Use actual registered Scene props. Host owns their native lifetimes."),
        ],
        build,
        None,
        None,
    )
}

pub(super) fn install(app: &mut App) {
    app.add_systems(
        Update,
        advance.after(agentelier_agent_host::ReceiveAgentEvents),
    );
}

fn build(
    world: &mut World,
    parent: Entity,
    definition: widget::Definition,
) -> Result<Entity, EcsError> {
    if definition.prepared.is_none() {
        return Err(EcsError::new(
            "invalid_scene_props",
            "脚本控件需要经发布工具预先检查",
        ));
    }
    let root = spawn_scene(world, bsn! {
        ChildOf(parent) Name("NativeScriptedWidget")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(6) }
    }).map_err(construction)?;
    let result =
        (|| {
            if validating_scene(world)
                && let Some(prepared) = definition.prepared
            {
                mount_view(world, root, prepared)?;
                return Ok(root);
            }
            let status = spawn_scene(world, bsn! {
            ChildOf(root) Name("ScriptedWidgetStatus") Text("正在构建脚本控件…") WidgetryText
            TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
        }).map_err(construction)?;
            world.entity_mut(root).insert(ScriptedWidget {
                status,
                load: Load::Queued(Box::new(definition)),
            });
            Ok(root)
        })();
    if result.is_err() {
        world.despawn(root);
    }
    result
}

fn mount_view(world: &mut World, root: Entity, view: widget::View) -> Result<Entity, EcsError> {
    let definition = SceneDefinition {
        template: "agentelier.composition".into(),
        props: Map::from_iter([("parts".into(), Value::Array(view.parts))]),
    };
    mount_scene_child(world, root, &definition)
}

fn advance(world: &mut World) {
    let mut ready = Vec::new();
    let mut start = Vec::new();
    for (entity, mut runtime) in world
        .query::<(Entity, &mut ScriptedWidget)>()
        .iter_mut(world)
    {
        match &runtime.load {
            Load::Queued(_) => {
                if let Load::Queued(definition) = std::mem::replace(&mut runtime.load, Load::Done) {
                    start.push((entity, runtime.status, definition));
                }
            }
            Load::Running(ticket) => {
                if let Some(result) = ticket.poll() {
                    ready.push((entity, runtime.status, result));
                    runtime.load = Load::Done;
                }
            }
            Load::Done => {}
        }
    }
    let host = world.resource::<AgentHost>().clone();
    for (root, status, definition) in start {
        let state = definition
            .prepared
            .as_ref()
            .map(|view| view.state.clone())
            .unwrap_or_default();
        match host.restore_widget(*definition, state) {
            Ok(ticket) => {
                if let Some(mut widget) = world.get_mut::<ScriptedWidget>(root) {
                    widget.load = Load::Running(ticket);
                }
            }
            Err(error) => fail(world, status, &error.to_string()),
        }
    }
    for (root, status, result) in ready {
        match result
            .map_err(|error| error.to_string())
            .and_then(|view| mount_view(world, root, view).map_err(|error| error.to_string()))
        {
            Ok(_) => {
                if let Some(mut node) = world.get_mut::<Node>(status) {
                    node.display = Display::None;
                }
            }
            Err(error) => fail(world, status, &error),
        }
    }
}

fn fail(world: &mut World, status: Entity, error: &str) {
    if let Some(mut text) = world.get_mut::<Text>(status) {
        text.0 = format!("脚本控件无法构建：{error}");
    }
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
