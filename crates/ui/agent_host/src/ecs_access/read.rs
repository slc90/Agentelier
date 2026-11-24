use super::{MAX_WORLD_INFO_ENTRIES, reflect_value};
use agentelier_bridge_connection::ecs_access::{EcsEntity, EcsError, Page, Query};
use bevy::ecs::{
    component::ComponentId,
    entity::EntityIndex,
    reflect::{AppTypeRegistry, ReflectComponent, ReflectResource},
    resource::IsResource,
};
use bevy::prelude::{ChildOf, Children, Entity, EntityRef, Name, World};
use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry, enums::VariantInfo};
use serde_json::{Value, json};

const MAX_PAGE: usize = 128;
const DEFAULT_PAGE: usize = 24;
const QUERY_SCAN_LIMIT: usize = 4096;
const MAX_METADATA_ITEMS: usize = 256;

type Result<T> = std::result::Result<T, EcsError>;

pub(super) fn registry(world: &World) -> Result<&AppTypeRegistry> {
    world
        .get_resource::<AppTypeRegistry>()
        .ok_or_else(|| EcsError::new("reflect_not_registered", "World 没有 Reflect 类型注册表"))
}

pub(super) fn registration<'a>(
    registry: &'a TypeRegistry,
    name: &str,
) -> Result<&'a TypeRegistration> {
    registry
        .get_with_type_path(name)
        .or_else(|| registry.get_with_short_type_path(name))
        .ok_or_else(|| {
            EcsError::new(
                "reflect_not_registered",
                format!("没有唯一的 Reflect 类型 {name}，请核对完整类型路径"),
            )
        })
}

pub(super) fn reflected<T: bevy::reflect::TypeData>(registration: &TypeRegistration) -> Result<&T> {
    registration.data::<T>().ok_or_else(|| {
        EcsError::new(
            "reflect_not_registered",
            format!(
                "{} 缺少相应的反射能力",
                registration.type_info().type_path()
            ),
        )
    })
}

pub(super) fn id(entity: EcsEntity) -> Result<Entity> {
    Entity::try_from_bits(entity.bits())
        .ok_or_else(|| EcsError::new("invalid_entity", "ECS 实体引用无效"))
}

pub(super) fn token(entity: Entity) -> String {
    EcsEntity::from_bits(entity.to_bits()).to_string()
}

pub(super) fn entity(world: &World, reference: EcsEntity) -> Result<EntityRef<'_>> {
    world
        .get_entity(id(reference)?)
        .ok()
        .filter(|entity| !entity.contains::<IsResource>())
        .ok_or_else(|| {
            EcsError::new(
                "entity_not_found",
                format!("实体 {reference} 已失效或不是普通实体"),
            )
        })
}

pub(super) fn exists(world: &World, reference: EcsEntity) -> Result<Value> {
    Ok(json!(
        world
            .get_entity(id(reference)?)
            .is_ok_and(|entity| !entity.contains::<IsResource>())
    ))
}

pub(super) fn parent(world: &World, reference: EcsEntity) -> Result<Value> {
    Ok(json!(
        entity(world, reference)?
            .get::<ChildOf>()
            .map(|child| token(child.parent()))
    ))
}

pub(super) fn children(world: &World, reference: EcsEntity, page: &Page) -> Result<Value> {
    let entity = entity(world, reference)?;
    let children = entity
        .get::<Children>()
        .map_or(&[][..], |children| &children[..]);
    let (start, end, next) = page_bounds(page, children.len())?;
    Ok(paged(
        children[start..end]
            .iter()
            .copied()
            .map(token)
            .collect::<Vec<_>>(),
        next,
    ))
}

pub(super) fn query(world: &World, filter: &Query) -> Result<Value> {
    let registry = registry(world)?.read();
    let resolve = |names: &[String]| -> Result<Vec<Option<ComponentId>>> {
        names
            .iter()
            .map(|name| component_id(world, &registry, name))
            .collect()
    };
    let with = resolve(&filter.with)?;
    let without = resolve(&filter.without)?;
    let limit = page_limit(filter.limit)?;
    let total = world.entities().len() as usize;
    let start = filter.cursor.unwrap_or(0).min(total);
    let end = total.min(start.saturating_add(QUERY_SCAN_LIMIT));
    let mut next = start;
    let mut results = Vec::new();
    for index in start..end {
        next = index + 1;
        let Some(index) = EntityIndex::from_raw_u32(index as u32) else {
            break;
        };
        let id = world.entities().resolve_from_index(index);
        let Ok(entity) = world.get_entity(id) else {
            continue;
        };
        if entity.contains::<IsResource>() {
            continue;
        }
        let archetype = entity.archetype();
        if with
            .iter()
            .all(|id| id.is_some_and(|id| archetype.contains(id)))
            && without
                .iter()
                .all(|id| id.is_none_or(|id| !archetype.contains(id)))
            && filter.name.as_ref().is_none_or(|name| {
                entity
                    .get::<Name>()
                    .is_some_and(|actual| actual.as_str() == name)
            })
        {
            results.push(token(id));
            if results.len() == limit {
                break;
            }
        }
    }
    Ok(paged(results, (next < total).then_some(next)))
}

fn component_id(world: &World, registry: &TypeRegistry, name: &str) -> Result<Option<ComponentId>> {
    if let Ok(registration) = registration(registry, name)
        && registration.data::<ReflectComponent>().is_some()
    {
        return Ok(world.components().get_id(registration.type_id()));
    }
    let mut found = None;
    for (id, info) in world
        .components()
        .iter_registered()
        .take(MAX_WORLD_INFO_ENTRIES)
    {
        let debug_name = info.name();
        let path = debug_name.as_string();
        if path == name {
            return Ok(Some(id));
        }
        if !path.contains('<') && path.rsplit("::").next() == Some(name) {
            if found.is_some() {
                return Err(EcsError::new(
                    "ambiguous_component",
                    format!("组件 {name} 不唯一，请使用完整路径"),
                ));
            }
            found = Some(id);
        }
    }
    found
        .map(Some)
        .ok_or_else(|| EcsError::new("component_not_registered", format!("未注册组件 {name}")))
}

pub(super) fn components(world: &World, reference: EcsEntity) -> Result<Value> {
    let entity = entity(world, reference)?;
    let ids = entity.archetype().components();
    if ids.len() > MAX_METADATA_ITEMS {
        return Err(metadata_limit());
    }
    let registry = registry(world)?.read();
    let names: Vec<_> = ids
        .iter()
        .filter_map(|id| world.components().get_info(*id))
        .map(|info| {
            if let Some(registration) = info.type_id().and_then(|id| registry.get(id)) {
                let paths = registration.type_info().type_path_table();
                if registry
                    .get_with_short_type_path(paths.short_path())
                    .is_some()
                {
                    return paths.short_path().to_owned();
                }
                return paths.path().to_owned();
            }
            info.name().to_string()
        })
        .collect();
    Ok(json!(names))
}

pub(super) fn component(world: &World, reference: EcsEntity, name: &str) -> Result<Value> {
    let entity = entity(world, reference)?;
    let registry = registry(world)?.read();
    let registration = registration(&registry, name)?;
    let component = reflected::<ReflectComponent>(registration)?;
    let value = component.reflect(entity).ok_or_else(|| {
        EcsError::new(
            "component_not_found",
            format!("实体 {reference} 没有组件 {name}"),
        )
    })?;
    reflect_value::encode(value, &registry)
}

pub(super) fn resource_entity(world: &World, registration: &TypeRegistration) -> Option<Entity> {
    let id = world.components().get_id(registration.type_id())?;
    world.resource_entities().get(id)
}

pub(super) fn resource(world: &World, name: &str) -> Result<Value> {
    let registry = registry(world)?.read();
    let registration = registration(&registry, name)?;
    reflected::<ReflectResource>(registration)?;
    let component = reflected::<ReflectComponent>(registration)?;
    let value = resource_entity(world, registration)
        .and_then(|id| world.get_entity(id).ok())
        .and_then(|entity| component.reflect(entity))
        .ok_or_else(|| EcsError::new("resource_not_found", format!("World 没有资源 {name}")))?;
    reflect_value::encode(value, &registry)
}

pub(super) fn types(world: &World, page: &Page, resources_only: bool) -> Result<Value> {
    let registry = registry(world)?.read();
    let mut types = registry
        .iter()
        .take(MAX_WORLD_INFO_ENTRIES + 1)
        .filter(|registration| {
            !resources_only
                || (registration.data::<ReflectResource>().is_some()
                    && resource_entity(world, registration).is_some())
        })
        .map(|registration| registration.type_info().type_path())
        .collect::<Vec<_>>();
    if types.len() > MAX_WORLD_INFO_ENTRIES {
        return Err(metadata_limit());
    }
    types.sort_unstable();
    let (start, end, next) = page_bounds(page, types.len())?;
    Ok(paged(&types[start..end], next))
}

pub(super) fn type_info(world: &World, name: &str) -> Result<Value> {
    let registry = registry(world)?.read();
    let registration = registration(&registry, name)?;
    let info = registration.type_info();
    let layout = match info {
        TypeInfo::Struct(info) => json!({"fields": named_fields(info.iter())?}),
        TypeInfo::TupleStruct(info) => json!({"fields": unnamed_fields(info.iter())?}),
        TypeInfo::Tuple(info) => json!({"fields": unnamed_fields(info.iter())?}),
        TypeInfo::Enum(info) => {
            if info.variant_len() > MAX_METADATA_ITEMS {
                return Err(metadata_limit());
            }
            let variants = info
                .iter()
                .map(|variant| {
                    let fields = match variant {
                        VariantInfo::Struct(info) => named_fields(info.iter())?,
                        VariantInfo::Tuple(info) => unnamed_fields(info.iter())?,
                        VariantInfo::Unit(_) => Vec::new(),
                    };
                    Ok(json!({"name": variant.name(), "fields": fields}))
                })
                .collect::<Result<Vec<_>>>()?;
            json!({"variants": variants})
        }
        TypeInfo::List(info) => json!({"item": info.item_ty().path()}),
        TypeInfo::Array(info) => json!({"item": info.item_ty().path(), "length": info.capacity()}),
        TypeInfo::Map(info) => {
            json!({"key": info.key_ty().path(), "value": info.value_ty().path()})
        }
        TypeInfo::Set(info) => json!({"item": info.value_ty().path()}),
        TypeInfo::Opaque(_) => Value::Null,
    };
    Ok(json!({
        "name": info.type_path(), "short_name": info.type_path_table().short_path(),
        "kind": info.kind().to_string(), "layout": layout,
        "component": registration.data::<ReflectComponent>().is_some() && registration.data::<ReflectResource>().is_none(),
        "resource": registration.data::<ReflectResource>().is_some(),
    }))
}

fn named_fields<'a>(
    fields: impl ExactSizeIterator<Item = &'a bevy::reflect::NamedField>,
) -> Result<Vec<Value>> {
    if fields.len() > MAX_METADATA_ITEMS {
        return Err(metadata_limit());
    }
    Ok(fields
        .map(|field| json!({"name": field.name(), "type": field.ty().path()}))
        .collect())
}

fn unnamed_fields<'a>(
    fields: impl ExactSizeIterator<Item = &'a bevy::reflect::UnnamedField>,
) -> Result<Vec<Value>> {
    if fields.len() > MAX_METADATA_ITEMS {
        return Err(metadata_limit());
    }
    Ok(fields
        .map(|field| json!({"index": field.index(), "type": field.ty().path()}))
        .collect())
}

fn page_limit(limit: Option<usize>) -> Result<usize> {
    let limit = limit.unwrap_or(DEFAULT_PAGE);
    if (1..=MAX_PAGE).contains(&limit) {
        return Ok(limit);
    }
    Err(EcsError::new(
        "invalid_ecs_arguments",
        "分页 limit 必须为 1 到 128",
    ))
}

fn page_bounds(page: &Page, size: usize) -> Result<(usize, usize, Option<usize>)> {
    let start = page.cursor.unwrap_or(0).min(size);
    let end = size.min(start.saturating_add(page_limit(page.limit)?));
    Ok((start, end, (end < size).then_some(end)))
}

fn paged(items: impl serde::Serialize, cursor: Option<usize>) -> Value {
    json!({"items": items, "cursor": cursor})
}

fn metadata_limit() -> EcsError {
    EcsError::new("ecs_result_limit", "ECS 类型元数据超过结果限制")
}
