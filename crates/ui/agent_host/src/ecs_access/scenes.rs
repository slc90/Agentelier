use super::read;
use agentelier_bridge_connection::ecs_access::{EcsEntity, EcsError, SceneDefinition, SceneTarget};
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, sync::Arc};

const MAX_SCENE_ENTITIES: usize = 128;
const MAX_SCENE_ATTACHMENTS: usize = 4096;

type Properties = Map<String, Value>;
type Result<T> = std::result::Result<T, EcsError>;
type Factory = Arc<dyn Fn(&mut World, Entity, &Properties) -> Result<Entity> + Send + Sync>;
type Patcher = Arc<dyn Fn(&mut World, Entity, &Properties) -> Result<()> + Send + Sync>;
type PrepareUpdate = Arc<dyn Fn(&World, Entity, &Properties) -> Result<Properties> + Send + Sync>;

#[derive(Clone)]
struct Template {
    description: &'static str,
    parameters: &'static [(&'static str, &'static str)],
    factory: Factory,
    patcher: Option<Patcher>,
    prepare: Option<PrepareUpdate>,
}

#[derive(Clone, Default, Resource)]
pub(crate) struct Registry(BTreeMap<&'static str, Template>);

#[derive(Component)]
struct Instance {
    template: &'static str,
    owned: Box<[Entity]>,
}

#[derive(Component)]
struct MountedScene(SceneTarget);

impl Registry {
    pub(crate) fn register<P: DeserializeOwned + Serialize + 'static, S: Scene>(
        &mut self,
        name: &'static str,
        description: &'static str,
        parameters: &'static [(&'static str, &'static str)],
        build: fn(P) -> bevy::prelude::Result<S>,
        update: Option<fn(&mut World, Entity, P) -> bevy::prelude::Result<()>>,
        prepare: Option<fn(&World, Entity, P) -> bevy::prelude::Result<P>>,
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
            patcher: update.map(|update| {
                let patcher: Patcher = Arc::new(move |world, root, properties| {
                    let properties = serde_json::from_value(Value::Object(properties.clone()))
                        .map_err(invalid_props)?;
                    update(world, root, properties)
                        .map_err(|error| EcsError::new("scene_update_failed", error.to_string()))
                });
                patcher
            }),
            prepare: prepare.map(|prepare| {
                let prepare: PrepareUpdate = Arc::new(move |world, root, properties| {
                    let properties = serde_json::from_value(Value::Object(properties.clone()))
                        .map_err(invalid_props)?;
                    let prepared = prepare(world, root, properties).map_err(invalid_props)?;
                    match serde_json::to_value(prepared).map_err(invalid_props)? {
                        Value::Object(properties) => Ok(properties),
                        _ => Err(invalid_props("Scene Props 必须是对象")),
                    }
                });
                prepare
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
            json!({"name": name, "description": template.description, "parameters": parameters, "supports_in_place_update": template.patcher.is_some()})
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

pub(super) fn validate(
    world: &mut World,
    definition: &SceneDefinition,
    target: Option<SceneTarget>,
) -> Result<Value> {
    let mut definition = definition.clone();
    if let Some(target) = target
        && let Some(prepare) = world
            .get_resource::<Registry>()
            .and_then(|registry| registry.0.get(definition.template.as_str()))
            .and_then(|template| template.prepare.clone())
    {
        let mut mounted = world.query::<(Entity, &MountedScene, &Instance)>();
        if let Some((root, _, _)) = mounted.iter(world).find(|(_, scene, instance)| {
            scene.0 == target && instance.template == definition.template
        }) {
            definition.props = prepare(world, root, &definition.props)?;
        }
    }
    definition.validate()?;
    let staging = bevy_widgetry::scene::spawn_scene(
        world,
        bsn! {
            Node { display: Display::None }
        },
    )
    .map_err(|error| EcsError::new("scene_validation_failed", error.to_string()))?;
    let result = build_native_scene(world, staging, &definition);
    world.despawn(staging);
    result?;
    serde_json::to_value(definition).map_err(invalid_props)
}

pub fn mount_native_scene(
    world: &mut World,
    parent: Entity,
    definition: &SceneDefinition,
    target: SceneTarget,
) -> Result<Entity> {
    let root = build_native_scene(world, parent, definition)?;
    world
        .get_entity_mut(root)
        .map_err(|_| EcsError::new("scene_construction_failed", "Scene 根实体失效"))?
        .insert(MountedScene(target));
    Ok(root)
}

fn build_native_scene(
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
    let (template, factory) = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.0.get_key_value(name))
        .map(|(name, template)| (*name, template.factory.clone()))
        .ok_or_else(|| {
            EcsError::new(
                "unknown_scene_template",
                format!("未登记 Scene 模板 {name}"),
            )
        })?;
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
        .insert(Instance { template, owned });
    Ok(root)
}

pub fn patch_native_scene(
    world: &mut World,
    root: Entity,
    definition: &SceneDefinition,
) -> Result<bool> {
    let Some(instance) = world
        .get::<Instance>(root)
        .filter(|instance| instance.template == definition.template)
    else {
        return Ok(false);
    };
    let patcher = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.0.get(instance.template))
        .and_then(|template| template.patcher.clone());
    let Some(patcher) = patcher else {
        return Ok(false);
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        patcher(world, root, &definition.props)
    }))
    .map_err(|_| EcsError::new("scene_update_failed", "Scene 更新异常终止"))??;
    Ok(true)
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
        .owned
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
