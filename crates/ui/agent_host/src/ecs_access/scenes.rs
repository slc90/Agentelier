use super::read;
use agentelier_bridge_connection::ecs_access::{EcsEntity, EcsError, SceneDefinition};
use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc};

const MAX_SCENE_ENTITIES: usize = 128;
const MAX_SCENE_ATTACHMENTS: usize = 4096;

type Properties = Map<String, Value>;
type Result<T> = std::result::Result<T, EcsError>;
type Factory = Arc<dyn Fn(&mut World, Entity, &Properties) -> Result<Entity> + Send + Sync>;

#[derive(Clone)]
struct Template {
    description: &'static str,
    parameters: &'static [(&'static str, &'static str)],
    factory: Factory,
}

#[derive(Clone, Default, Resource)]
pub(crate) struct Registry(BTreeMap<&'static str, Template>);

#[derive(Component)]
struct Instance(Box<[Entity]>);

impl Registry {
    pub(crate) fn register<P: DeserializeOwned + 'static, S: Scene>(
        &mut self,
        name: &'static str,
        description: &'static str,
        parameters: &'static [(&'static str, &'static str)],
        build: fn(P) -> bevy::prelude::Result<S>,
    ) -> Result<()> {
        let std::collections::btree_map::Entry::Vacant(entry) = self.0.entry(name) else {
            return Err(EcsError::new(
                "duplicate_scene_template",
                format!("Scene 模板 {name} 已登记"),
            ));
        };
        entry.insert(Template {
            description,
            parameters,
            factory: Arc::new(move |world, parent, properties| {
                let properties = serde_json::from_value(Value::Object(properties.clone()))
                    .map_err(invalid_props)?;
                let scene = build(properties).map_err(invalid_props)?;
                bevy_widgetry::scene::spawn_scene(world, bsn! { @scene ChildOf(parent) })
                    .map_err(|error| EcsError::new("scene_construction_failed", error.to_string()))
            }),
        });
        Ok(())
    }
}

pub(super) fn templates(world: &World) -> Value {
    let Some(registry) = world.get_resource::<Registry>() else {
        return json!([]);
    };
    let templates: Vec<_> = registry
        .0
        .iter()
        .map(|(name, template)| {
            let parameters: BTreeMap<_, _> = template.parameters.iter().copied().collect();
            json!({"name": name, "description": template.description, "parameters": parameters})
        })
        .collect();
    json!(templates)
}

pub(super) fn spawn(
    world: &mut World,
    name: &str,
    parent: EcsEntity,
    props: &Properties,
) -> Result<Value> {
    let parent = read::entity(world, parent)?.id();
    instance(world, name, parent, props).map(|root| json!(read::token(root)))
}

pub(super) fn validate(world: &mut World, definition: &SceneDefinition) -> Result<Value> {
    definition.validate()?;
    let staging = bevy_widgetry::scene::spawn_scene(
        world,
        bsn! {
            Node { display: Display::None }
        },
    )
    .map_err(|error| EcsError::new("scene_validation_failed", error.to_string()))?;
    let result = mount_native_scene(world, staging, definition);
    world.despawn(staging);
    result.map(|_| json!(true))
}

pub fn mount_native_scene(
    world: &mut World,
    parent: Entity,
    definition: &SceneDefinition,
) -> Result<Entity> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        instance(world, &definition.template, parent, &definition.props)
    }))
    .map_err(|_| EcsError::new("scene_construction_failed", "Scene 构造异常终止"))?
}

fn instance(world: &mut World, name: &str, parent: Entity, props: &Properties) -> Result<Entity> {
    let factory = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.0.get(name))
        .ok_or_else(|| {
            EcsError::new(
                "unknown_scene_template",
                format!("未登记 Scene 模板 {name}"),
            )
        })?
        .factory
        .clone();
    if world.get::<Node>(parent).is_none() {
        return Err(EcsError::new(
            "invalid_scene_parent",
            "Scene 父实体必须是 Bevy UI Node",
        ));
    }
    let root = factory(world, parent, props)?;
    let owned = match owned_entities(world, root) {
        Ok(owned) => owned,
        Err(error) => {
            world.despawn(root);
            return Err(error);
        }
    };
    world
        .get_entity_mut(root)
        .map_err(|_| EcsError::new("scene_construction_failed", "Scene 根实体在构造期间失效"))?
        .insert(Instance(owned));
    Ok(root)
}

pub(super) fn despawn(world: &mut World, root: EcsEntity) -> Result<Value> {
    let root = read::entity(world, root)?;
    let owned = root
        .get::<Instance>()
        .ok_or_else(|| {
            EcsError::new(
                "scene_instance_not_found",
                "目标并非已登记 Scene 实例的根实体",
            )
        })?
        .0
        .clone();
    let external: Vec<_> = owned
        .iter()
        .filter_map(|id| world.get::<Children>(*id))
        .flatten()
        .copied()
        .filter(|child| !owned.contains(child))
        .take(MAX_SCENE_ATTACHMENTS + 1)
        .collect();
    if external.len() > MAX_SCENE_ATTACHMENTS {
        return Err(EcsError::new(
            "scene_size_limit",
            "Scene 实例的外部子实体过多",
        ));
    }
    for child in external {
        if let Ok(mut entity) = world.get_entity_mut(child) {
            entity.remove::<ChildOf>();
        }
    }
    for id in owned.iter().copied().rev() {
        if world.entities().contains_spawned(id) {
            world.despawn(id);
        }
    }
    Ok(json!(true))
}

fn owned_entities(world: &World, root: Entity) -> Result<Box<[Entity]>> {
    let mut owned = vec![root];
    let mut index = 0;
    while let Some(entity) = owned.get(index).copied() {
        if let Some(children) = world.get::<Children>(entity) {
            if owned.len() + children.len() > MAX_SCENE_ENTITIES {
                return Err(EcsError::new(
                    "scene_size_limit",
                    "Scene 模板生成的实体过多",
                ));
            }
            owned.extend(children);
        }
        index += 1;
    }
    Ok(owned.into_boxed_slice())
}

fn invalid_props(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}
