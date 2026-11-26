use super::parameters::{Binding, Definition, Parameters};
use agentelier_agent_host::{
    AgentHostPlugin, EcsError, SceneDefinition, SceneState, SceneStored, mount_scene_child,
    prepare_scene_definition, replace_scene_children,
};
use bevy::prelude::*;
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText, theme::WidgetryTheme};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

const TEMPLATE: &str = "agentelier.composition";
const MAX_PARTS: usize = 16;
const MAX_ID_BYTES: usize = 64;
const MAX_TEXT_BYTES: usize = 8192;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    parts: Vec<Placement>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, Definition>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Placement {
    id: String,
    area: Area,
    scene: SceneDefinition,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    bindings: Vec<Binding>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Area {
    Intro,
    Controls,
    Content,
    Footer,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TextProperties {
    text: String,
}

#[derive(Component, FromTemplate, Clone, Copy)]
struct Areas {
    intro: Entity,
    controls: Entity,
    content: Entity,
    footer: Entity,
}

impl Areas {
    fn iter(self) -> [(Area, Entity); 4] {
        [
            (Area::Intro, self.intro),
            (Area::Controls, self.controls),
            (Area::Content, self.content),
            (Area::Footer, self.footer),
        ]
    }
}

#[derive(Clone)]
struct Part {
    placement: Placement,
    root: Entity,
}

#[derive(Component, Clone)]
struct Composition(Vec<Part>);

#[derive(Component, Reflect)]
#[reflect(Component)]
struct CompositionPart {
    id: String,
}

type SceneResult<T> = std::result::Result<T, EcsError>;

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene(
        "agentelier.text",
        "Display plain native text in a reply or as part of a composition.",
        &[("text", "Required nonblank UTF-8 string, at most 8192 bytes.")],
        text,
        None,
        None,
    )?
    .with_ecs_scene_factory(
        TEMPLATE,
        "Arrange registered native scenes in one reply artifact, with stable part IDs and semantic areas.",
        &[
            ("parts", "Required array of 1 to 16 objects {id, area, scene, bindings?}. id is a unique stable string of up to 64 bytes. area is intro, controls, content, or footer. scene is {template, props}. Bindings are objects with kind input, text, visible, series_filter, or reset. Each except reset requires parameter. text optionally accepts format value or square. series_filter routes choice values to a scatter chart's series names and accepts an optional all value. reset is for buttons."),
            ("parameters", "Optional object keyed by parameter name. Each value is {domain, default, current?}. domain is {kind, ...}: integer/float with min,max,step; boolean; choice with options [{value,label}]; or text with max_length. A bound input takes its range/options/initial value from the parameter. Omit current on update only to retain an eligible current value."),
        ],
        build,
        Some(update),
        Some(prepare),
    )?
    .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

fn restore(mut props: Properties, state: &SceneState) -> Result<Properties> {
    restore_values(&mut props.parameters, &state.values)?;
    Ok(props)
}

fn prepare_unmounted(mut props: Properties, stored: &SceneStored) -> Result<Properties> {
    let mut previous: Properties =
        serde_json::from_value(serde_json::Value::Object(stored.definition.props.clone()))?;
    if let Some(values) = &stored.values {
        restore_values(&mut previous.parameters, values)?;
    }
    Parameters::new(previous.parameters)?.retain(&mut props.parameters)?;
    Ok(props)
}

fn restore_values(
    definitions: &mut BTreeMap<String, Definition>,
    values: &serde_json::Map<String, serde_json::Value>,
) -> Result {
    if definitions.len() != values.len() {
        return Err(io::Error::other("保存的 Scene 参数集合与定义不匹配").into());
    }
    for (name, definition) in definitions {
        let value = values
            .get(name)
            .ok_or_else(|| io::Error::other(format!("保存的 Scene 状态缺少参数 {name}")))?;
        definition.restore_current(name, value)?;
    }
    Ok(())
}

pub(super) fn install(app: &mut App) {
    app.register_type::<CompositionPart>();
}

fn text(props: TextProperties) -> Result<impl Scene> {
    let TextProperties { text } = props;
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        return Err(io::Error::other("text 必须非空且最多 8192 字节").into());
    }
    Ok(bsn! {
        Text(text) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)
        Node { width: percent(100), min_width: px(0) }
    })
}

fn build(world: &mut World, parent: Entity, props: Properties) -> SceneResult<Entity> {
    let (placements, parameters) = props.materialize().map_err(invalid)?;
    let root = spawn_scene(world, bsn! {
        Name("NativeComposition") ChildOf(parent)
        Node {
            width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column,
            row_gap: px(10), padding: UiRect::all(px(12)),
            border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)),
        }
        template(|context| Ok(BorderColor::all(context.resource::<WidgetryTheme>().text_field.editable.normal.border)))
        Areas { intro: #SceneIntro, controls: #SceneControls, content: #SceneContent, footer: #SceneFooter }
        Children [
            #SceneIntro @area()--
            #SceneControls @area()--
            #SceneContent @area()--
            #SceneFooter @area()
        ]
    }).map_err(construction)?;
    let result = (|| {
        let areas = get_areas(world, root)?;
        let parts = placements
            .into_iter()
            .map(|placement| mount_part(world, root, placement))
            .collect::<SceneResult<Vec<_>>>()?;
        arrange(world, areas, &parts);
        attach(world, root, parts, parameters);
        Ok(root)
    })();
    if result.is_err() {
        world.despawn(root);
    }
    result
}

fn area() -> impl Scene {
    bsn! {
        Node { display: Display::None, width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(8) }
    }
}

impl Properties {
    fn validate(&self) -> Result {
        if self.parts.is_empty() || self.parts.len() > MAX_PARTS {
            return Err(io::Error::other("parts 必须包含 1 至 16 个子 Scene").into());
        }
        let mut ids = BTreeSet::new();
        for part in &self.parts {
            if part.id.trim().is_empty() || part.id.len() > MAX_ID_BYTES {
                return Err(io::Error::other("子 Scene id 必须非空且最多 64 字节").into());
            }
            if !ids.insert(part.id.as_str()) {
                return Err(io::Error::other(format!("子 Scene id 重复：{}", part.id)).into());
            }
            if part.scene.template == TEMPLATE {
                return Err(io::Error::other("组合区只接受基础 Scene 模板").into());
            }
        }
        Ok(())
    }

    fn materialize(self) -> Result<(Vec<Placement>, Parameters)> {
        self.validate()?;
        let parameters = Parameters::new(self.parameters)?;
        let mut parts = self.parts;
        for part in &mut parts {
            parameters.specialize(&mut part.scene, &part.bindings)?;
        }
        Ok((parts, parameters))
    }
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    props.validate()?;
    if let Some(parameters) = world.get::<Parameters>(root) {
        parameters.retain(&mut props.parameters)?;
    }
    let current = composition(world, root)?;
    for placement in &mut props.parts {
        if let Some(existing) = current
            .0
            .iter()
            .find(|part| part.placement.id == placement.id)
            && placement.scene != existing.placement.scene
            && !placement
                .bindings
                .iter()
                .any(|binding| matches!(binding, Binding::Input { .. }))
        {
            prepare_scene_definition(world, existing.root, &mut placement.scene)
                .map_err(|error| part_error(&placement.id, error))?;
        }
    }
    Ok(props)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result {
    let (placements, parameters) = props.materialize()?;
    let areas = get_areas(world, root)?;
    let existing = composition(world, root)?.0.clone();
    let staging = spawn_scene(
        world,
        bsn! { ChildOf(root) Node { display: Display::None } },
    )
    .map_err(|error| error.into_bevy_error())?;
    let result = update_parts(
        world, root, areas, staging, &existing, placements, parameters,
    );
    world.despawn(staging);
    result.map_err(Into::into)
}

fn update_parts(
    world: &mut World,
    root: Entity,
    areas: Areas,
    staging: Entity,
    existing: &[Part],
    placements: Vec<Placement>,
    parameters: Parameters,
) -> SceneResult<()> {
    let mut parts = Vec::with_capacity(placements.len());
    let mut new = Vec::new();
    for placement in placements {
        if let Some(part) = existing.iter().find(|part| {
            part.placement.id == placement.id
                && part.placement.scene == placement.scene
                && part.placement.bindings == placement.bindings
                && world.entities().contains_spawned(part.root)
                && world
                    .get::<Parameters>(root)
                    .is_some_and(|before| parameters.can_reuse(before, &placement.bindings))
        }) {
            parts.push(Part {
                placement,
                root: part.root,
            });
        } else {
            let part = mount_part(world, staging, placement)?;
            new.push(part.root);
            parts.push(part);
        }
    }
    let retired: Vec<_> = existing
        .iter()
        .filter(|part| parts.iter().all(|next| next.root != part.root))
        .map(|part| part.root)
        .collect();
    replace_scene_children(world, root, &retired, &new)?;
    arrange(world, areas, &parts);
    attach(world, root, parts, parameters);
    Ok(())
}

fn attach(world: &mut World, root: Entity, parts: Vec<Part>, parameters: Parameters) {
    let bindings = parts
        .iter()
        .map(|part| (part.root, part.placement.bindings.clone()));
    parameters.attach(world, root, bindings);
    world.entity_mut(root).insert(Composition(parts));
}

fn mount_part(world: &mut World, parent: Entity, placement: Placement) -> SceneResult<Part> {
    let root = mount_scene_child(world, parent, &placement.scene)
        .map_err(|error| part_error(&placement.id, error))?;
    world.entity_mut(root).insert(CompositionPart {
        id: placement.id.clone(),
    });
    Ok(Part { placement, root })
}

fn arrange(world: &mut World, areas: Areas, parts: &[Part]) {
    for (area, parent) in areas.iter() {
        for part in parts.iter().filter(|part| part.placement.area == area) {
            if world
                .get::<ChildOf>(part.root)
                .is_none_or(|current| current.parent() != parent)
            {
                world.entity_mut(part.root).insert(ChildOf(parent));
            }
        }
    }
    for (area, parent) in areas.iter() {
        let mut ordered: Vec<_> = parts
            .iter()
            .filter(|part| part.placement.area == area)
            .map(|part| part.root)
            .collect();
        ordered.extend(
            world
                .get::<Children>(parent)
                .into_iter()
                .flatten()
                .copied()
                .filter(|child| parts.iter().all(|part| part.root != *child)),
        );
        let same_order = world
            .get::<Children>(parent)
            .into_iter()
            .flatten()
            .copied()
            .eq(ordered.iter().copied());
        let mut region = world.entity_mut(parent);
        if !same_order {
            region.replace_children(&ordered);
        }
        if let Some(mut node) = region.get_mut::<Node>() {
            let display = if ordered.is_empty() {
                Display::None
            } else {
                Display::Flex
            };
            if node.display != display {
                node.display = display;
            }
        }
    }
}

fn get_areas(world: &World, root: Entity) -> SceneResult<Areas> {
    world
        .get::<Areas>(root)
        .copied()
        .filter(|areas| {
            areas
                .iter()
                .iter()
                .all(|(_, entity)| world.get::<Node>(*entity).is_some())
        })
        .ok_or_else(|| construction("原生组合缺少布局区域"))
}

fn composition(world: &World, root: Entity) -> Result<&Composition> {
    world
        .get::<Composition>(root)
        .ok_or_else(|| io::Error::other("原生组合缺少子场景记录").into())
}

fn invalid(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}

fn part_error(id: &str, error: EcsError) -> EcsError {
    EcsError::new(error.code, format!("子 Scene {id}：{}", error.message))
}
