use super::{read, reflect_value};
use agentelier_bridge_connection::ecs_access::{EcsEntity, EcsError};
use bevy::ecs::{
    reflect::{ReflectComponent, ReflectResource},
    resource::IsResource,
};
use bevy::prelude::{ChildOf, Children, World};
use bevy::reflect::{
    PartialReflect, Reflect, ReflectMut, ReflectRef, TypeRegistration, TypeRegistry,
};
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};

const MAX_SPAWN_COMPONENTS: usize = 32;
const MAX_ANCESTORS: usize = 1024;

type Result<T> = std::result::Result<T, EcsError>;

#[derive(Clone, Copy)]
pub(super) enum Target {
    Component(EcsEntity),
    Resource,
}

pub(super) enum Change<'a> {
    Insert(&'a Value),
    Set(&'a Value),
    Patch(&'a Map<String, Value>),
    Remove,
}

pub(super) fn spawn(world: &mut World, components: &Map<String, Value>) -> Result<Value> {
    if components.len() > MAX_SPAWN_COMPONENTS {
        return Err(EcsError::new(
            "ecs_input_limit",
            "新实体最多接受 32 个初始组件",
        ));
    }
    let registry = read::registry(world)?.clone();
    let registry = registry.read();
    let mut seen = HashSet::new();
    let prepared = components
        .iter()
        .map(|(name, value)| {
            let registration = read::registration(&registry, name)?;
            let component = writable(registration, false)?;
            if !seen.insert(registration.type_id()) {
                return Err(EcsError::new(
                    "duplicate_component",
                    format!("初始组件 {name} 重复"),
                ));
            }
            Ok((
                component,
                reflect_value::decode(value, registration, &registry)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;

    let id = world.spawn_empty().id();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        for (component, value) in prepared {
            let mut entity = world
                .get_entity_mut(id)
                .map_err(|_| despawned_during_spawn())?;
            component.insert(&mut entity, value.as_partial_reflect(), &registry);
        }
        world.get_entity(id).map_err(|_| despawned_during_spawn())?;
        Ok(json!(read::token(id)))
    }));
    match outcome {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => {
            if world.entities().contains_spawned(id) {
                world.despawn(id);
            }
            Err(error)
        }
        Err(panic) => {
            if world.entities().contains_spawned(id) {
                world.despawn(id);
            }
            resume_unwind(panic)
        }
    }
}

fn despawned_during_spawn() -> EcsError {
    EcsError::new("entity_not_found", "ECS 组件的生命周期操作已销毁新实体")
}

pub(super) fn change(
    world: &mut World,
    target: Target,
    name: &str,
    change: Change<'_>,
) -> Result<Value> {
    let registry = read::registry(world)?.clone();
    let registry = registry.read();
    let target_is_resource = matches!(target, Target::Resource);
    let own_id = match target {
        Target::Component(reference) => Some(read::entity(world, reference)?.id()),
        Target::Resource => None,
    };
    let registration = read::registration(&registry, name)?;
    let component = writable(registration, target_is_resource)?;
    let id = own_id.or_else(|| read::resource_entity(world, registration));
    let existing = id
        .and_then(|id| world.get_entity(id).ok())
        .and_then(|entity| component.reflect(entity));
    let missing = || {
        let code = if target_is_resource {
            "resource_not_found"
        } else {
            "component_not_found"
        };
        EcsError::new(code, format!("目标没有 {name}"))
    };

    match change {
        Change::Insert(value) | Change::Set(value) => {
            if matches!(change, Change::Set(_)) && existing.is_none() {
                return Err(missing());
            }
            let value = reflect_value::decode(value, registration, &registry)?;
            let mut entity = if let Some(id) = id {
                world.entity_mut(id)
            } else {
                world.spawn_empty()
            };
            component.insert(&mut entity, value.as_partial_reflect(), &registry);
            Ok(json!(true))
        }
        Change::Patch(fields) => {
            let existing = existing.ok_or_else(missing)?;
            let id = id.ok_or_else(missing)?;
            let mutable = world
                .components()
                .get_id(registration.type_id())
                .and_then(|id| world.components().get_info(id))
                .is_some_and(|info| info.mutable());
            if !mutable {
                return Err(EcsError::new(
                    "immutable_component",
                    format!("{name} 无法原位修改"),
                ));
            }
            let patch = prepare_fields(existing.as_partial_reflect(), fields, &registry)?;
            if !patch.is_empty() {
                let mut entity = world.entity_mut(id);
                let mut value = component.reflect_mut(&mut entity).ok_or_else(missing)?;
                for (key, field) in patch {
                    let field_ref = field_mut(value.as_partial_reflect_mut(), &key)
                        .and_then(PartialReflect::try_as_reflect_mut)
                        .ok_or_else(|| invalid_field(&key))?;
                    field_ref.set(field).map_err(|_| invalid_field(&key))?;
                }
            }
            Ok(json!({"fields": fields.keys().collect::<Vec<_>>() }))
        }
        Change::Remove => {
            existing.ok_or_else(missing)?;
            let id = id.ok_or_else(missing)?;
            if target_is_resource {
                let component_id = world
                    .components()
                    .get_id(registration.type_id())
                    .ok_or_else(missing)?;
                if !world.remove_resource_by_id(component_id) {
                    return Err(missing());
                }
            } else {
                component.remove(&mut world.entity_mut(id));
            }
            Ok(json!(true))
        }
    }
}

fn writable(registration: &TypeRegistration, resource: bool) -> Result<&ReflectComponent> {
    let info = registration.type_info();
    if resource {
        read::reflected::<ReflectResource>(registration)?;
    } else if registration.data::<ReflectResource>().is_some()
        || info.is::<IsResource>()
        || info.is::<ChildOf>()
        || info.is::<Children>()
    {
        return Err(EcsError::new(
            "ecs_managed_type",
            format!("{} 由专用 Resource 或关系操作维护", info.type_path()),
        ));
    }
    let path = info.type_path();
    if path.starts_with("bevy_widgetry_") || (resource && path.starts_with("agentelier_")) {
        return Err(EcsError::new(
            "ecs_managed_type",
            format!("{path} 由控件或应用业务维护"),
        ));
    }
    read::reflected::<ReflectComponent>(registration)
}

fn prepare_fields(
    value: &dyn PartialReflect,
    fields: &Map<String, Value>,
    registry: &TypeRegistry,
) -> Result<Vec<(String, Box<dyn Reflect>)>> {
    fields
        .iter()
        .map(|(key, input)| {
            let field = field(value, key)
                .and_then(PartialReflect::try_as_reflect)
                .ok_or_else(|| invalid_field(key))?;
            let registration = registry
                .get(field.reflect_type_info().type_id())
                .ok_or_else(|| {
                    EcsError::new("reflect_not_registered", format!("字段 {key} 的类型未注册"))
                })?;
            let value = reflect_value::decode(input, registration, registry)
                .map_err(|error| EcsError::new(error.code, format!("字段 {key}: {error}")))?;
            Ok((key.clone(), value))
        })
        .collect()
}

fn field<'a>(value: &'a dyn PartialReflect, key: &str) -> Option<&'a dyn PartialReflect> {
    match value.reflect_ref() {
        ReflectRef::Struct(value) => value.field(key),
        ReflectRef::TupleStruct(value) => value.field(key.parse().ok()?),
        ReflectRef::Tuple(value) => value.field(key.parse().ok()?),
        _ => None,
    }
}

fn field_mut<'a>(
    value: &'a mut dyn PartialReflect,
    key: &str,
) -> Option<&'a mut dyn PartialReflect> {
    match value.reflect_mut() {
        ReflectMut::Struct(value) => value.field_mut(key),
        ReflectMut::TupleStruct(value) => value.field_mut(key.parse().ok()?),
        ReflectMut::Tuple(value) => value.field_mut(key.parse().ok()?),
        _ => None,
    }
}

fn invalid_field(key: &str) -> EcsError {
    EcsError::new("invalid_reflect_field", format!("无法修改反射字段 {key}"))
}

pub(super) fn despawn(world: &mut World, reference: EcsEntity) -> Result<Value> {
    let id = read::entity(world, reference)?.id();
    Ok(json!(world.despawn(id)))
}

pub(super) fn reparent(
    world: &mut World,
    reference: EcsEntity,
    parent: EcsEntity,
) -> Result<Value> {
    let child = read::entity(world, reference)?.id();
    let parent = read::entity(world, parent)?.id();
    let mut ancestor = Some(parent);
    for _ in 0..MAX_ANCESTORS {
        let Some(id) = ancestor else {
            world.entity_mut(parent).add_child(child);
            return Ok(json!(true));
        };
        if id == child {
            return Err(EcsError::new("hierarchy_cycle", "父子关系会形成循环"));
        }
        ancestor = world.get::<ChildOf>(id).map(ChildOf::parent);
    }
    Err(EcsError::new(
        "hierarchy_limit",
        "关系链超过 ECS 祖先检查上限",
    ))
}

pub(super) fn unparent(world: &mut World, reference: EcsEntity) -> Result<Value> {
    let id = read::entity(world, reference)?.id();
    world.entity_mut(id).remove::<ChildOf>();
    Ok(json!(true))
}
