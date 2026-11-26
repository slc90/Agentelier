mod model;
mod render;

use super::{Board, Drawing, Gesture, construction, plot};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition, SceneState, SceneStored};
use agentelier_markdown::{MarkdownSource, MarkdownView};
use bevy::{
    prelude::*,
    ui::{UiSystems, Val2},
};
use bevy_widgetry::scene::spawn_scene;
use model::{Model, Properties};
use serde_json::{Map, Value};
use std::io;

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.graph_diagram";
const GESTURE: Gesture = Gesture { pick, adjust };

#[derive(Component)]
struct Graph {
    board: Board,
    model: Model,
    formulas: Vec<Entity>,
    source: Entity,
}

#[derive(Clone, Copy, Component)]
struct FormulaAt {
    canvas: Entity,
    at: Vec2,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw a native graph or commutative diagram with stable named nodes, real graph edges and optional typeset mathematical labels.",
        &[
            ("title", "Optional nonblank title."),
            ("kind", "graph (default) or commutative. Edges in a commutative diagram are directed as specified; the renderer does not verify commutativity."),
            ("nodes", "1 to 24 {id,label?,tex?,at?:[x,y],draggable?}. id is unique; tex when present is displayed on the node. Coordinates are [0,1] from left to right and top to bottom. Unplaced nodes use circular or grid layout."),
            ("edges", "Up to 64 {from,to,directed?,label?,tex?}, referring to node IDs. Graph edges default to undirected, while commutative edges default to directed. Optional tex is the mathematical edge label. Parallel edges and self-loops remain separate."),
            ("source_tex", "Optional original TeX, preserved as readable and copyable source alongside the structured graph."),
        ],
        build, None, Some(prepare),
    )?.with_conditional_ecs_scene_update(TEMPLATE, patch)?
      .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, place_formula_labels.after(UiSystems::Layout));
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let model = Model::new(props).map_err(super::super::invalid)?;
    let root = plot::root(world, parent, "NativeGraphDiagram", model.title.as_deref())?;
    let result = (|| {
        let (drawing, labels) = model.drawing();
        let board = Board::mount(
            world,
            root,
            model.title.clone(),
            drawing,
            model.interactive().then_some(GESTURE),
        )?;
        let frame = board.frame;
        let formulas = labels.into_iter().map(|(tex, at)| {
            let anchor = FormulaAt { canvas: board.canvas, at };
            let markdown = format!("${tex}$");
            spawn_scene(world, bsn! {
                ChildOf(frame) Name("GraphMathLabel") Pickable::IGNORE
                Node { position_type: PositionType::Absolute, top: percent(50), left: percent(50), width: px(160), min_width: px(0) }
                template(move |_| Ok(anchor))
                Children [ @MarkdownView { @source: markdown, @source_controls: false } Pickable::IGNORE ]
            }).map_err(construction)
        }).collect::<Result<Vec<_>, _>>()?;
        let source_text = model.source();
        let has_source = model.source_tex.is_some();
        let source = spawn_scene(world, bsn! {
            ChildOf(root) Name("GraphSource") @MarkdownView { @source: source_text, @source_controls: has_source }
        }).map_err(construction)?;
        world.entity_mut(root).insert(Graph {
            board,
            model,
            formulas,
            source,
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
    let graph = world
        .get::<Graph>(root)
        .ok_or_else(|| io::Error::other("图缺少节点状态"))?;
    if graph.model.title != model.title
        || graph.model.interactive() != model.interactive()
        || graph.model.source_tex.is_some() != model.source_tex.is_some()
    {
        return Ok(false);
    }
    let (drawing, formulas) = model.drawing();
    let (_, old_formulas) = graph.model.drawing();
    if graph.board.labels.len() != drawing.labels.len()
        || !formulas
            .iter()
            .map(|(tex, _)| tex)
            .eq(old_formulas.iter().map(|(tex, _)| tex))
    {
        return Ok(false);
    }
    let (board, labels, source) = (graph.board.clone(), graph.formulas.clone(), graph.source);
    apply(world, &board, &labels, drawing, formulas)?;
    board.reset_input(world);
    world
        .get_mut::<MarkdownSource>(source)
        .ok_or_else(|| io::Error::other("图缺少源文"))?
        .set_if_neq(MarkdownSource(model.source()));
    world.entity_mut(root).insert(Graph {
        board,
        model,
        formulas: labels,
        source,
    });
    Ok(true)
}

fn apply(
    world: &mut World,
    board: &Board,
    anchors: &[Entity],
    drawing: Drawing,
    formulas: Vec<(String, Vec2)>,
) -> Result {
    board.plot(world, drawing)?;
    for (&entity, (_, at)) in anchors.iter().zip(formulas) {
        let mut anchor = world
            .get_mut::<FormulaAt>(entity)
            .ok_or_else(|| io::Error::other("图缺少数学标签锚点"))?;
        if anchor.at != at {
            anchor.at = at;
        }
    }
    Ok(())
}

fn pick(world: &World, root: Entity, at: Vec2, tolerance: f32) -> Option<(usize, Vec2)> {
    world.get::<Graph>(root)?.model.pick(at, tolerance)
}

fn adjust(world: &mut World, root: Entity, index: usize, at: Vec2) -> Result<bool> {
    let (board, anchors, (drawing, formulas)) = {
        let Some(mut graph) = world.get_mut::<Graph>(root) else {
            return Ok(false);
        };
        if !graph.model.move_node(index, at) {
            return Ok(false);
        }
        (
            graph.board.clone(),
            graph.formulas.clone(),
            graph.model.drawing(),
        )
    };
    apply(world, &board, &anchors, drawing, formulas)?;
    Ok(true)
}

fn place_formula_labels(
    mut overlays: Query<(Entity, &FormulaAt, &mut UiTransform)>,
    sizes: Query<&ComputedNode>,
    children: Query<&Children>,
    names: Query<&Name>,
) {
    for (entity, anchor, mut transform) in &mut overlays {
        let Ok(canvas) = sizes.get(anchor.canvas) else {
            continue;
        };
        let Ok(wrapper) = sizes.get(entity) else {
            continue;
        };
        let space = canvas.size() * canvas.inverse_scale_factor();
        if space.min_element() <= 0.0 {
            continue;
        }
        let zoom = (space / Vec2::new(656.0, 291.0))
            .min_element()
            .clamp(0.1, 10.0);
        let wrapper = wrapper.size() * wrapper.inverse_scale_factor();
        let actual = children
            .iter_descendants(entity)
            .find(|&e| {
                names
                    .get(e)
                    .is_ok_and(|name| name.as_str() == "MarkdownInlineFormulaImage")
            })
            .and_then(|e| sizes.get(e).ok())
            .map_or(Vec2::new(42.0, wrapper.y), |layout| {
                layout.size() * layout.inverse_scale_factor()
            });
        let left_top = anchor.at * zoom - (wrapper + (actual - wrapper) * zoom) * 0.5;
        let pose = UiTransform {
            translation: Val2::px(left_top.x, left_top.y),
            scale: Vec2::splat(zoom),
            ..default()
        };
        transform.set_if_neq(pose);
    }
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if let Some(values) = snapshot(world, root) {
        props.use_values(&values, false)?;
    }
    Ok(props)
}

fn restore(mut props: Properties, saved: &SceneState) -> Result<Properties> {
    props.use_values(&saved.values, true)?;
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

pub(super) fn snapshot(world: &World, root: Entity) -> Option<Map<String, Value>> {
    let values = world.get::<Graph>(root)?.model.saved_values();
    (!values.is_empty()).then_some(values)
}

pub(super) fn restore_definition(
    scene: &mut SceneDefinition,
    values: &Map<String, Value>,
    overwrite: bool,
) -> Result {
    let mut props: Properties = serde_json::from_value(Value::Object(scene.props.clone()))?;
    props.use_values(values, overwrite)?;
    scene.props = serde_json::from_value(serde_json::to_value(props)?)?;
    Ok(())
}

pub(super) fn inherit_definition(scene: &mut SceneDefinition, old: &SceneDefinition) -> Result {
    let previous: Properties = serde_json::from_value(Value::Object(old.props.clone()))?;
    restore_definition(scene, &previous.saved_values(), false)
}
