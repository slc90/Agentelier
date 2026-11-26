mod model;
mod render;

use super::{Board, Drawing, Gesture, construction, plot};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition, SceneState, SceneStored};
use agentelier_markdown::{MarkdownSource, MarkdownView};
use bevy::prelude::*;
use bevy_widgetry::scene::spawn_scene;
use model::{Model, Properties};
use serde_json::{Map, Value};
use std::io;

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.geometry_diagram";

#[derive(Component)]
struct Geometry {
    board: Board,
    model: Model,
    details: Entity,
}

const GESTURE: Gesture = Gesture { pick, adjust };

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw semantic planar geometry: draggable points, line segments, circles, angles, polygon areas and vector components.",
        &[
            ("title", "Optional nonblank title."),
            ("bounds", "Required [[min_x,max_x],[min_y,max_y]] math coordinate ranges. Both directions use the same display scale."),
            ("points", "1 to 32 unique point definitions {id,label?,formula?,draggable?,position}. position is {kind:'free',at:[x,y]}, {kind:'on_circle',center:point_id,radius:positive,angle:degrees}, or {kind:'on_segment',from:point_id,to:point_id,t:0..1}. Define referenced points in any order. formula is optional original TeX, without delimiters."),
            ("objects", "Up to 64 semantic objects. {kind:'segment',from,to,arrow?}; {kind:'circle',center,radius}; {kind:'angle',a,vertex,b}; {kind:'polygon',points:[point_id,...],area?}; {kind:'decomposition',from,to}. All point references must exist. 'area' shows absolute shoelace area, or reports no single region at a crossing."),
        ],
        build, None, Some(prepare),
    )?.with_conditional_ecs_scene_update(TEMPLATE, patch)?
      .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let model = Model::new(props).map_err(super::super::invalid)?;
    let root = plot::root(
        world,
        parent,
        "NativeGeometryDiagram",
        model.title.as_deref(),
    )?;
    let result = (|| {
        let drawing = model.drawing();
        let board = Board::mount(
            world,
            root,
            model.title.clone(),
            drawing,
            model.interactive().then_some(GESTURE),
        )?;
        let details = model.details();
        let details = spawn_scene(world, bsn! {
            ChildOf(root) Name("GeometryMathLabels") @MarkdownView { @source: details, @source_controls: false }
        }).map_err(construction)?;
        world.entity_mut(root).insert(Geometry {
            board,
            model,
            details,
        });
        Ok(root)
    })();
    if result.is_err() {
        world.despawn(root);
    }
    result
}

fn patch(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let model = Model::new(props)?;
    let live = world
        .get::<Geometry>(root)
        .ok_or_else(|| io::Error::other("缺少几何示意状态"))?;
    if live.model.title != model.title || live.model.interactive() != model.interactive() {
        return Ok(false);
    }
    let drawing = model.drawing();
    if drawing.paths.len() != live.board.paths.len()
        || drawing.labels.len() != live.board.labels.len()
    {
        return Ok(false);
    }
    let board = live.board.clone();
    let details = live.details;
    apply(world, &board, details, drawing, &model)?;
    board.reset_input(world);
    world.entity_mut(root).insert(Geometry {
        board,
        model,
        details,
    });
    Ok(true)
}

fn apply(
    world: &mut World,
    board: &Board,
    details: Entity,
    drawing: Drawing,
    model: &Model,
) -> Result {
    board.plot(world, drawing)?;
    world
        .get_mut::<MarkdownSource>(details)
        .ok_or_else(|| io::Error::other("缺少几何数学标签"))?
        .set_if_neq(MarkdownSource(model.details()));
    Ok(())
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if let Some(values) = snapshot(world, root) {
        props.use_values(&values, false)?;
    }
    Ok(props)
}

fn restore(mut props: Properties, state: &SceneState) -> Result<Properties> {
    props.use_values(&state.values, true)?;
    Ok(props)
}

fn prepare_unmounted(mut props: Properties, old: &SceneStored) -> Result<Properties> {
    if let Some(values) = &old.values {
        props.use_values(values, false)?;
    } else {
        let previous: Properties =
            serde_json::from_value(Value::Object(old.definition.props.clone()))?;
        props.use_values(&previous.saved_values(), false)?;
    }
    Ok(props)
}

pub(in crate::agent_scenes) fn snapshot(world: &World, root: Entity) -> Option<Map<String, Value>> {
    let state = world.get::<Geometry>(root)?;
    let values = state.model.saved_values();
    (!values.is_empty()).then_some(values)
}

pub(in crate::agent_scenes) fn restore_definition(
    scene: &mut SceneDefinition,
    values: &Map<String, Value>,
    overwrite: bool,
) -> Result {
    if scene.template == TEMPLATE {
        let mut props: Properties = serde_json::from_value(Value::Object(scene.props.clone()))?;
        props.use_values(values, overwrite)?;
        scene.props = serde_json::from_value(serde_json::to_value(props)?)?;
    }
    Ok(())
}

pub(in crate::agent_scenes) fn inherit_definition(
    scene: &mut SceneDefinition,
    previous: &SceneDefinition,
) -> Result {
    if scene.template == TEMPLATE && previous.template == TEMPLATE {
        let old: Properties = serde_json::from_value(Value::Object(previous.props.clone()))?;
        restore_definition(scene, &old.saved_values(), false)?;
    }
    Ok(())
}

fn pick(world: &World, root: Entity, point: Vec2, radius: f32) -> Option<(usize, Vec2)> {
    world.get::<Geometry>(root)?.model.pick(point, radius)
}

fn adjust(world: &mut World, root: Entity, index: usize, point: Vec2) -> Result<bool> {
    let (board, drawing) = {
        let Some(mut geometry) = world.get_mut::<Geometry>(root) else {
            return Ok(false);
        };
        let target = geometry.model.plane.unmap(point);
        if !geometry.model.move_point(index, target)? {
            return Ok(false);
        }
        (geometry.board.clone(), geometry.model.drawing())
    };
    board.plot(world, drawing)?;
    Ok(true)
}
