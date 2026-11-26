use super::read;
use agentelier_bridge_connection::ecs_access::{
    EcsEntity, EcsError, SceneDefinition, SceneStored, SceneTarget,
};
use agentelier_bridge_connection::widget;
use agentelier_conversation_bridge::SceneState;
use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    sync::Arc,
};

const MAX_SCENE_ENTITIES: usize = 128;
const MAX_SCENE_ATTACHMENTS: usize = 4096;

type Properties = Map<String, Value>;
type Result<T> = std::result::Result<T, EcsError>;
type Factory = Arc<dyn Fn(&mut World, Entity, &Properties) -> Result<Entity> + Send + Sync>;
type Patcher = Arc<dyn Fn(&mut World, Entity, &Properties) -> Result<bool> + Send + Sync>;
type PrepareUpdate = Arc<dyn Fn(&World, Entity, &Properties) -> Result<Properties> + Send + Sync>;
type Restore = Arc<dyn Fn(&Properties, &SceneState) -> Result<Properties> + Send + Sync>;
type PrepareUnmounted = Arc<dyn Fn(&Properties, &SceneStored) -> Result<Properties> + Send + Sync>;

#[derive(Clone)]
struct Template {
    description: &'static str,
    parameters: &'static [(&'static str, &'static str)],
    factory: Factory,
    patcher: Option<Patcher>,
    prepare: Option<PrepareUpdate>,
    restore: Option<Restore>,
    prepare_unmounted: Option<PrepareUnmounted>,
}

#[derive(Clone, Default, Resource)]
pub(crate) struct Registry(BTreeMap<&'static str, Template>);

#[derive(Component)]
struct Instance {
    template: &'static str,
    owned: Box<[Entity]>,
}

#[derive(Component, Clone, Copy)]
pub struct MountedScene {
    pub target: SceneTarget,
    pub revision: u64,
}

#[derive(Component)]
pub struct WidgetLiveState(pub widget::State);

#[derive(Default, Resource)]
pub(crate) struct WidgetCandidates(HashMap<SceneTarget, CheckedWidget>);

struct CheckedWidget {
    owner: Entity,
    revision: u64,
    properties: Properties,
    program: widget::Program,
}

pub(crate) fn prune_widget_candidates(
    mut candidates: ResMut<WidgetCandidates>,
    mounted: Query<&MountedScene>,
) {
    candidates.0.retain(|target, candidate| {
        mounted.get(candidate.owner).is_ok_and(|mount| {
            mount.target == *target && mount.revision.checked_add(1) == Some(candidate.revision)
        })
    });
}

pub fn take_widget_candidate(
    world: &mut World,
    root: Entity,
    definition: &widget::Definition,
) -> Result<Option<widget::Program>> {
    let Some(&mount) = world.get::<MountedScene>(root) else {
        return Ok(None);
    };
    let properties = serde_json::to_value(definition).map_err(invalid_props)?;
    let mut candidates = world.resource_mut::<WidgetCandidates>();
    let matched = candidates.0.get(&mount.target).is_some_and(|candidate| {
        candidate.owner == root
            && mount.revision.checked_add(1) == Some(candidate.revision)
            && properties.as_object() == Some(&candidate.properties)
    });
    Ok(matched
        .then(|| candidates.0.remove(&mount.target))
        .flatten()
        .map(|candidate| candidate.program))
}

fn mounted_widget(
    world: &mut World,
    target: SceneTarget,
    revision: u64,
) -> Option<(Entity, &WidgetLiveState)> {
    world
        .query::<(Entity, &MountedScene, &WidgetLiveState)>()
        .iter(world)
        .find_map(|(root, mount, state)| {
            (mount.target == target && mount.revision == revision).then_some((root, state))
        })
}

pub(super) fn widget_state(world: &mut World, target: SceneTarget, revision: u64) -> Result<Value> {
    Ok(
        mounted_widget(world, target, revision).map_or(Value::Null, |(_, snapshot)| {
            Value::Object(snapshot.0.clone())
        }),
    )
}

impl Registry {
    pub(crate) fn register<P: DeserializeOwned + Serialize + 'static>(
        &mut self,
        name: &'static str,
        description: &'static str,
        parameters: &'static [(&'static str, &'static str)],
        build: impl Fn(&mut World, Entity, P) -> Result<Entity> + Send + Sync + 'static,
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
                build(world, parent, properties)
            }),
            patcher: update.map(|update| {
                let patcher: Patcher = Arc::new(move |world, root, properties| {
                    let properties = serde_json::from_value(Value::Object(properties.clone()))
                        .map_err(invalid_props)?;
                    update(world, root, properties)
                        .map(|()| true)
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
            restore: None,
            prepare_unmounted: None,
        });
        Ok(())
    }

    pub(crate) fn register_conditional_update<P: DeserializeOwned + 'static>(
        &mut self,
        name: &'static str,
        update: fn(&mut World, Entity, P) -> bevy::prelude::Result<bool>,
    ) -> Result<()> {
        let template = self.0.get_mut(name).ok_or_else(|| {
            EcsError::new(
                "unknown_scene_template",
                format!("未登记 Scene 模板 {name}"),
            )
        })?;
        template.patcher = Some(Arc::new(move |world, root, properties| {
            let properties =
                serde_json::from_value(Value::Object(properties.clone())).map_err(invalid_props)?;
            update(world, root, properties)
                .map_err(|error| EcsError::new("scene_update_failed", error.to_string()))
        }));
        Ok(())
    }

    pub(crate) fn register_restore<P: DeserializeOwned + Serialize + 'static>(
        &mut self,
        name: &'static str,
        restore: fn(P, &SceneState) -> bevy::prelude::Result<P>,
        prepare_unmounted: fn(P, &SceneStored) -> bevy::prelude::Result<P>,
    ) -> Result<()> {
        let template = self.0.get_mut(name).ok_or_else(|| {
            EcsError::new(
                "unknown_scene_template",
                format!("未登记 Scene 模板 {name}"),
            )
        })?;
        template.restore = Some(Arc::new(move |properties, state| {
            let properties =
                serde_json::from_value(Value::Object(properties.clone())).map_err(invalid_props)?;
            let restored = restore(properties, state).map_err(invalid_props)?;
            match serde_json::to_value(restored).map_err(invalid_props)? {
                Value::Object(properties) => Ok(properties),
                _ => Err(invalid_props("Scene Props 必须是对象")),
            }
        }));
        template.prepare_unmounted = Some(Arc::new(move |properties, stored| {
            let properties =
                serde_json::from_value(Value::Object(properties.clone())).map_err(invalid_props)?;
            let prepared = prepare_unmounted(properties, stored).map_err(invalid_props)?;
            match serde_json::to_value(prepared).map_err(invalid_props)? {
                Value::Object(properties) => Ok(properties),
                _ => Err(invalid_props("Scene Props 必须是对象")),
            }
        }));
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
    stored: Option<&SceneStored>,
    widget: Option<&widget::Candidate>,
) -> Result<Value> {
    let mut definition = definition.clone();
    if let Some(target) = target {
        let mut mounted = world.query::<(Entity, &MountedScene, &Instance)>();
        if let Some((root, _, _)) = mounted.iter(world).find(|(_, scene, instance)| {
            scene.target == target && instance.template == definition.template
        }) {
            prepare_scene_definition(world, root, &mut definition)?;
        } else if let Some(stored) =
            stored.filter(|stored| stored.definition.template == definition.template)
            && let Some(prepare) = world
                .get_resource::<Registry>()
                .and_then(|registry| registry.0.get(definition.template.as_str()))
                .and_then(|template| template.prepare_unmounted.as_ref())
        {
            definition.props = prepare(&definition.props, stored)?;
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
    world.insert_resource(SceneValidation);
    let result = build_native_scene(world, staging, &definition);
    world.remove_resource::<SceneValidation>();
    world.despawn(staging);
    result?;
    if let (Some(target), Some(candidate)) = (target, widget) {
        let current = mounted_widget(world, target, candidate.expected_revision);
        if let Some((owner, current)) = current {
            if candidate.captured.as_ref() != Some(&current.0) {
                return Err(EcsError::new(
                    "widget_state_changed",
                    "脚本控件状态在校验期间发生变化，请按当前状态重新修订",
                ));
            }
            if let Some(revision) = candidate.expected_revision.checked_add(1) {
                world.resource_mut::<WidgetCandidates>().0.insert(
                    target,
                    CheckedWidget {
                        owner,
                        revision,
                        properties: definition.props.clone(),
                        program: candidate.program.clone(),
                    },
                );
            }
        }
    }
    serde_json::to_value(definition).map_err(invalid_props)
}

#[derive(Resource)]
struct SceneValidation;

pub fn validating_scene(world: &World) -> bool {
    world.contains_resource::<SceneValidation>()
}

pub fn prepare_scene_definition(
    world: &World,
    root: Entity,
    definition: &mut SceneDefinition,
) -> Result<()> {
    if let Some(instance) = world.get::<Instance>(root)
        && instance.template == definition.template
        && let Some(prepare) = world
            .get_resource::<Registry>()
            .and_then(|registry| registry.0.get(instance.template))
            .and_then(|template| template.prepare.as_ref())
    {
        definition.props = prepare(world, root, &definition.props)?;
    }
    Ok(())
}

pub fn mount_scene_child(
    world: &mut World,
    parent: Entity,
    definition: &SceneDefinition,
) -> Result<Entity> {
    definition.validate()?;
    build_native_scene(world, parent, definition)
}

pub fn mount_native_scene(
    world: &mut World,
    parent: Entity,
    definition: &SceneDefinition,
    mount: MountedScene,
) -> Result<Entity> {
    let root = build_native_scene(world, parent, definition)?;
    world
        .get_entity_mut(root)
        .map_err(|_| EcsError::new("scene_construction_failed", "Scene 根实体失效"))?
        .insert(mount);
    Ok(root)
}

pub fn restore_scene_definition(
    world: &World,
    definition: &SceneDefinition,
    state: &SceneState,
) -> Result<SceneDefinition> {
    let restore = world
        .get_resource::<Registry>()
        .and_then(|registry| registry.0.get(definition.template.as_str()))
        .and_then(|template| template.restore.as_ref())
        .ok_or_else(|| invalid_props("此 Scene 模板不支持恢复用户状态"))?;
    let mut definition = definition.clone();
    definition.props = restore(&definition.props, state)?;
    Ok(definition)
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
    .map_err(|_| EcsError::new("scene_update_failed", "Scene 更新异常终止"))?
}

pub(super) fn despawn(world: &mut World, root: EcsEntity) -> Result<Value> {
    let root = read::entity(world, root)?.id();
    let owned = instance_ownership(world, root)?.to_vec();
    retire_entities(world, &owned)?;
    Ok(json!(true))
}

pub fn replace_scene_children(
    world: &mut World,
    root: Entity,
    retiring: &[Entity],
    incoming: &[Entity],
) -> Result<()> {
    let owned = instance_ownership(world, root)?;
    let mut retired = Vec::new();
    for &child in retiring {
        if child == root || !owned.contains(&child) {
            return Err(EcsError::new(
                "scene_instance_not_found",
                "要替换的子 Scene 不属于当前实例",
            ));
        }
        retired.extend_from_slice(instance_ownership(world, child)?);
    }
    let remove: BTreeSet<_> = retired.iter().copied().collect();
    let mut next: Vec<_> = owned
        .iter()
        .copied()
        .filter(|id| !remove.contains(id))
        .collect();
    for &child in incoming {
        next.extend_from_slice(instance_ownership(world, child)?);
    }
    if next.len() > MAX_SCENE_ENTITIES {
        return Err(EcsError::new(
            "scene_size_limit",
            "Scene 模板生成的实体过多",
        ));
    }
    retire_entities(world, &retired)?;
    let Some(mut ownership) = world.get_mut::<Instance>(root) else {
        return Err(EcsError::new(
            "scene_instance_not_found",
            "当前 Scene 实例已失效",
        ));
    };
    ownership.owned = next.into_boxed_slice();
    Ok(())
}

fn instance_ownership(world: &World, root: Entity) -> Result<&[Entity]> {
    world
        .get::<Instance>(root)
        .map(|instance| instance.owned.as_ref())
        .ok_or_else(|| {
            EcsError::new(
                "scene_instance_not_found",
                "目标并非已登记 Scene 实例的根实体",
            )
        })
}

fn retire_entities(world: &mut World, owned: &[Entity]) -> Result<()> {
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
    Ok(())
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

pub(crate) fn invalid_props(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}
