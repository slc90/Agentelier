pub(crate) mod dispatch;
mod read;
mod reflect_value;
pub(crate) mod scenes;
mod write;

use crate::AgentHost;
use agentelier_bridge_connection::ecs_access::{EcsError, EcsOperation};
use bevy::ecs::error::Severity;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};
use write::{Change, Target};

const MAX_REQUESTS_PER_UPDATE: usize = 8;
const MAX_WORLD_INFO_ENTRIES: usize = 4096;

pub(super) fn process(world: &mut World) -> Result {
    let host = world.resource::<AgentHost>().clone();
    for _ in 0..MAX_REQUESTS_PER_UPDATE {
        let Some(request) = host.next_ecs_request()? else {
            break;
        };
        if let Err(error) = request.check() {
            request.respond(Err(error));
            continue;
        }
        match catch_unwind(AssertUnwindSafe(|| execute(world, &request.operation))) {
            Ok(result) => request.respond(result),
            Err(_) => {
                let error = EcsError::new("ecs_operation_panicked", "主线程 ECS 操作异常终止");
                request.respond(Err(error.clone()));
                return Err(BevyError::from(error).with_severity(Severity::Error));
            }
        }
    }
    Ok(())
}

fn execute(world: &mut World, operation: &EcsOperation) -> std::result::Result<Value, EcsError> {
    match operation {
        EcsOperation::WorldInfo => {
            let archetypes = world.archetypes();
            let entities: usize = archetypes
                .iter()
                .take(MAX_WORLD_INFO_ENTRIES)
                .map(|archetype| archetype.entities().len())
                .sum();
            let reflected = world
                .get_resource::<AppTypeRegistry>()
                .map_or(0, |registry| {
                    registry
                        .read()
                        .iter()
                        .take(MAX_WORLD_INFO_ENTRIES + 1)
                        .count()
                });
            Ok(json!({
                "entities": entities,
                "entities_complete": archetypes.len() <= MAX_WORLD_INFO_ENTRIES,
                "registered_ecs_ids": world.components().len(),
                "reflect_types": reflected.min(MAX_WORLD_INFO_ENTRIES),
                "reflect_types_complete": reflected <= MAX_WORLD_INFO_ENTRIES,
                "dispatch": dispatch::overview(world),
                "operations": [
                    "ecs_world_info()",
                    "reflect_types([#{limit, cursor}]) -> #{items, cursor}",
                    "reflect_type(name)",
                    "ecs_query(#{\"with\": [\"Node\"], without: [], name: ..., limit: 24, cursor: ...}) -> #{items, cursor}",
                    "ecs_exists(entity)", "ecs_components(entity)", "ecs_get(entity, type)",
                    "ecs_parent(entity)", "ecs_children(entity[, #{limit, cursor}]) -> #{items, cursor}",
                    "ecs_resources([#{limit, cursor}]) -> #{items, cursor}", "ecs_resource_get(type)",
                    "ecs_spawn(#{TypeName: value}) -> entity", "ecs_insert(entity, type, value)",
                    "ecs_patch(entity, type, fields)", "ecs_remove(entity, type)", "ecs_despawn(entity)",
                    "ecs_reparent(entity, parent)", "ecs_unparent(entity)",
                    "ecs_resource_insert(type, value)", "ecs_resource_set(type, value)",
                    "ecs_resource_patch(type, fields)", "ecs_resource_remove(type)",
                    "ecs_trigger(type, payload)", "ecs_write_message(type, payload)",
                    "scene_templates()", "scene_spawn(name, parent, props) -> entity", "scene_despawn(root)",
                ],
            }))
        }
        EcsOperation::Query(filter) => read::query(world, filter),
        EcsOperation::Exists(entity) => read::exists(world, *entity),
        EcsOperation::Parent(entity) => read::parent(world, *entity),
        EcsOperation::Children(entity, page) => read::children(world, *entity, page),
        EcsOperation::ReflectTypes(page) => read::types(world, page, false),
        EcsOperation::ReflectType(name) => read::type_info(world, name),
        EcsOperation::Components(entity) => read::components(world, *entity),
        EcsOperation::Get(entity, name) => read::component(world, *entity, name),
        EcsOperation::Resources(page) => read::types(world, page, true),
        EcsOperation::ResourceGet(name) => read::resource(world, name),
        EcsOperation::Spawn(components) => write::spawn(world, components),
        EcsOperation::Insert(entity, name, value) => write::change(
            world,
            Target::Component(*entity),
            name,
            Change::Insert(value),
        ),
        EcsOperation::Patch(entity, name, fields) => write::change(
            world,
            Target::Component(*entity),
            name,
            Change::Patch(fields),
        ),
        EcsOperation::Remove(entity, name) => {
            write::change(world, Target::Component(*entity), name, Change::Remove)
        }
        EcsOperation::Despawn(entity) => write::despawn(world, *entity),
        EcsOperation::Reparent(entity, parent) => write::reparent(world, *entity, *parent),
        EcsOperation::Unparent(entity) => write::unparent(world, *entity),
        EcsOperation::ResourceInsert(name, value) => {
            write::change(world, Target::Resource, name, Change::Insert(value))
        }
        EcsOperation::ResourceSet(name, value) => {
            write::change(world, Target::Resource, name, Change::Set(value))
        }
        EcsOperation::ResourcePatch(name, fields) => {
            write::change(world, Target::Resource, name, Change::Patch(fields))
        }
        EcsOperation::ResourceRemove(name) => {
            write::change(world, Target::Resource, name, Change::Remove)
        }
        EcsOperation::Trigger(name, payload) => dispatch::event(world, name, payload),
        EcsOperation::WriteMessage(name, payload) => dispatch::message(world, name, payload),
        EcsOperation::SceneTemplates => Ok(scenes::templates(world)),
        EcsOperation::SceneValidate {
            definition,
            target,
            stored,
        } => scenes::validate(world, definition, *target, stored.as_ref()),
        EcsOperation::SceneSpawn(name, parent, props) => scenes::spawn(world, name, *parent, props),
        EcsOperation::SceneDespawn(root) => scenes::despawn(world, *root),
    }
}
