mod input;

use agentelier_agent_host::{
    AgentHost, AgentHostPlugin, EcsError, SceneDefinition, mount_scene_child, patch_native_scene,
    replace_scene_children, validating_scene, widget,
};
use bevy::prelude::*;
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText};
use serde_json::{Map, Value};
use std::{collections::VecDeque, io};

#[derive(Component)]
struct ScriptedWidget {
    status: Entity,
    load: Load,
}

enum Load {
    Queued(Box<widget::Definition>),
    Building(widget::Ticket),
    Live(Box<Live>),
    Done,
}

struct Live {
    native: Entity,
    program: widget::Program,
    view: widget::View,
    events: VecDeque<widget::Event>,
    pending: Option<widget::Ticket>,
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
            ("on_event callback", "Optional fn on_event(ctx, event), called locally in real input order; return () to ignore or return a new #{parts, state}. ctx.state holds the last successfully displayed state. event.part is a stable part ID. event.kind is click, number, selection, text, key, or drag. Number and text carry value and is_final; selection carries value; key carries key, pressed, repeat; drag carries phase (start/move/end), position and delta."),
        ],
        build,
        None,
        None,
    )
}

pub(super) fn install(app: &mut App) {
    input::install(app);
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
    mount_scene_child(world, root, &layout(view.parts))
}

fn layout(parts: Vec<Value>) -> SceneDefinition {
    SceneDefinition {
        template: "agentelier.composition".into(),
        props: Map::from_iter([("parts".into(), Value::Array(parts))]),
    }
}

fn advance(world: &mut World) {
    let roots: Vec<_> = world
        .query::<(Entity, &ScriptedWidget)>()
        .iter(world)
        .filter_map(|(entity, state)| {
            let work = match &state.load {
                Load::Queued(_) | Load::Building(_) => true,
                Load::Live(live) => live.pending.is_some() || !live.events.is_empty(),
                Load::Done => false,
            };
            work.then_some(entity)
        })
        .collect();
    if roots.is_empty() {
        return;
    }
    let host = world.resource::<AgentHost>().clone();
    for root in roots {
        let (status, load) = {
            let Some(mut state) = world.get_mut::<ScriptedWidget>(root) else {
                continue;
            };
            (state.status, std::mem::replace(&mut state.load, Load::Done))
        };
        let next = match load {
            Load::Queued(definition) => begin(&host, *definition).unwrap_or_else(|error| {
                feedback(world, status, Some(format!("脚本控件无法构建：{error}")));
                Load::Done
            }),
            Load::Building(ticket) => match ticket.poll() {
                None => Load::Building(ticket),
                Some(reply) => match reply.map_err(|error| error.to_string()).and_then(|reply| {
                    mount_live(world, root, reply).map_err(|error| error.to_string())
                }) {
                    Ok(live) => {
                        feedback(world, status, None);
                        Load::Live(Box::new(live))
                    }
                    Err(error) => {
                        feedback(world, status, Some(format!("脚本控件无法构建：{error}")));
                        Load::Done
                    }
                },
            },
            Load::Live(mut live) => {
                run_event(world, status, &mut live, &host);
                Load::Live(live)
            }
            Load::Done => Load::Done,
        };
        if let Some(mut state) = world.get_mut::<ScriptedWidget>(root) {
            state.load = next;
        }
    }
}

fn begin(host: &AgentHost, definition: widget::Definition) -> Result<Load> {
    let state = definition
        .prepared
        .as_ref()
        .map(|view| view.state.clone())
        .unwrap_or_default();
    host.run_widget(widget::Job::restore(definition, state))
        .map(Load::Building)
}

fn mount_live(world: &mut World, root: Entity, reply: widget::Reply) -> Result<Live> {
    let view = reply
        .view
        .ok_or_else(|| io::Error::other("脚本控件没有返回原生定义"))?;
    let native = mount_scene_child(world, root, &layout(view.parts.clone()))?;
    if let Err(error) = replace_scene_children(world, root, &[], &[native]) {
        world.despawn(native);
        return Err(error.into());
    }
    Ok(Live {
        native,
        program: reply.program,
        view,
        events: VecDeque::new(),
        pending: None,
    })
}

fn run_event(world: &mut World, status: Entity, live: &mut Live, host: &AgentHost) {
    if let Some(ticket) = &live.pending {
        let Some(result) = ticket.poll() else {
            return;
        };
        live.pending = None;
        let applied = result.map_err(|error| error.to_string()).and_then(|reply| {
            if let Some(view) = reply.view {
                if view.parts != live.view.parts {
                    let definition = layout(view.parts.clone());
                    definition.validate().map_err(|error| error.to_string())?;
                    let updated = patch_native_scene(world, live.native, &definition)
                        .map_err(|error| error.to_string())?;
                    if !updated {
                        return Err("脚本控件原生布局已失效".to_owned());
                    }
                }
                live.view = view;
                feedback(world, status, None);
            }
            Ok(())
        });
        if let Err(error) = applied {
            feedback(world, status, Some(format!("脚本控件事件失败：{error}")));
        }
    }
    if let Some(event) = live.events.front().cloned() {
        let job = widget::Job::event(live.program.clone(), live.view.state.clone(), event);
        match host.run_widget(job) {
            Ok(ticket) => {
                live.pending = Some(ticket);
                live.events.pop_front();
            }
            Err(error) => feedback(
                world,
                status,
                Some(format!("脚本控件暂时无法处理输入：{error}")),
            ),
        }
    }
}

fn feedback(world: &mut World, status: Entity, error: Option<String>) {
    let display = if error.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    if let Some(mut node) = world.get_mut::<Node>(status) {
        node.display = display;
    }
    if let Some(mut text) = world.get_mut::<Text>(status) {
        text.0 = error.unwrap_or_default();
    }
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
