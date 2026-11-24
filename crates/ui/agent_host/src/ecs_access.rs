mod read;
mod reflect_value;

use crate::AgentHost;
use agentelier_bridge_connection::ecs_access::{EcsError, EcsOperation};
use bevy::ecs::error::Severity;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use serde_json::{Value, json};
use std::panic::{AssertUnwindSafe, catch_unwind};

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
                "operations": [
                    "ecs_world_info()",
                    "reflect_types([#{limit, cursor}]) -> #{items, cursor}",
                    "reflect_type(name)",
                    "ecs_query(#{\"with\": [\"Node\"], without: [], name: ..., limit: 24, cursor: ...}) -> #{items, cursor}",
                    "ecs_exists(entity)", "ecs_components(entity)", "ecs_get(entity, type)",
                    "ecs_parent(entity)", "ecs_children(entity[, #{limit, cursor}]) -> #{items, cursor}",
                    "ecs_resources([#{limit, cursor}]) -> #{items, cursor}", "ecs_resource_get(type)",
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
    }
}
