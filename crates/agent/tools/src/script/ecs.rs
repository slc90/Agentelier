use super::Control;
use crate::{
    ToolFailure,
    ecs_access::{EcsAccess, EcsEntity, EcsError, EcsOperation, Page, Query},
};
use rhai::{Array, Dynamic, Engine, EvalAltResult, ImmutableString, Map, NativeCallContext};
use serde::de::DeserializeOwned;
use serde_json::Value;

const MAX_NAME_BYTES: usize = 1024;
const MAX_FILTER_TYPES: usize = 16;
const MAX_PAYLOAD_BYTES: usize = 32 * 1024;
const MAX_PAYLOAD_DEPTH: usize = 16;
const VALUE_OVERHEAD: usize = 16;
type NativeResult = Result<Dynamic, Box<EvalAltResult>>;

#[derive(Clone)]
pub(super) struct Run {
    pub(super) access: EcsAccess,
    pub(super) control: Control,
}

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("ecs_world_info", |ctx: NativeCallContext<'_>| {
            call(ctx, Ok(EcsOperation::WorldInfo))
        })
        .register_fn("reflect_types", |ctx: NativeCallContext<'_>| {
            call(ctx, Ok(EcsOperation::ReflectTypes(Page::default())))
        })
        .register_fn(
            "reflect_types",
            |ctx: NativeCallContext<'_>, options: Map| {
                call(ctx, from_map(options).map(EcsOperation::ReflectTypes))
            },
        )
        .register_fn(
            "reflect_type",
            |ctx: NativeCallContext<'_>, name: ImmutableString| {
                call(ctx, type_name(name).map(EcsOperation::ReflectType))
            },
        )
        .register_fn("ecs_query", |ctx: NativeCallContext<'_>, filter: Map| {
            call(ctx, query(filter).map(EcsOperation::Query))
        })
        .register_fn(
            "ecs_exists",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(ctx, EcsEntity::parse(&entity).map(EcsOperation::Exists))
            },
        )
        .register_fn(
            "ecs_components",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(ctx, EcsEntity::parse(&entity).map(EcsOperation::Components))
            },
        )
        .register_fn(
            "ecs_get",
            |ctx: NativeCallContext<'_>, entity: ImmutableString, name: ImmutableString| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Get(
                            EcsEntity::parse(&entity)?,
                            type_name(name)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_parent",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(ctx, EcsEntity::parse(&entity).map(EcsOperation::Parent))
            },
        )
        .register_fn(
            "ecs_children",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(
                    ctx,
                    EcsEntity::parse(&entity)
                        .map(|entity| EcsOperation::Children(entity, Page::default())),
                )
            },
        )
        .register_fn(
            "ecs_children",
            |ctx: NativeCallContext<'_>, entity: ImmutableString, options: Map| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Children(
                            EcsEntity::parse(&entity)?,
                            from_map(options)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn("ecs_resources", |ctx: NativeCallContext<'_>| {
            call(ctx, Ok(EcsOperation::Resources(Page::default())))
        })
        .register_fn(
            "ecs_resources",
            |ctx: NativeCallContext<'_>, options: Map| {
                call(ctx, from_map(options).map(EcsOperation::Resources))
            },
        )
        .register_fn(
            "ecs_resource_get",
            |ctx: NativeCallContext<'_>, name: ImmutableString| {
                call(ctx, type_name(name).map(EcsOperation::ResourceGet))
            },
        )
        .register_fn(
            "ecs_spawn",
            |ctx: NativeCallContext<'_>, components: Map| {
                call(ctx, from_map(components).map(EcsOperation::Spawn))
            },
        )
        .register_fn(
            "ecs_insert",
            |ctx: NativeCallContext<'_>,
             entity: ImmutableString,
             name: ImmutableString,
             value: Dynamic| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Insert(
                            EcsEntity::parse(&entity)?,
                            type_name(name)?,
                            from_value(value)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_patch",
            |ctx: NativeCallContext<'_>,
             entity: ImmutableString,
             name: ImmutableString,
             fields: Map| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Patch(
                            EcsEntity::parse(&entity)?,
                            type_name(name)?,
                            from_map(fields)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_remove",
            |ctx: NativeCallContext<'_>, entity: ImmutableString, name: ImmutableString| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Remove(
                            EcsEntity::parse(&entity)?,
                            type_name(name)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_despawn",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(ctx, EcsEntity::parse(&entity).map(EcsOperation::Despawn))
            },
        )
        .register_fn(
            "ecs_reparent",
            |ctx: NativeCallContext<'_>, entity: ImmutableString, parent: ImmutableString| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::Reparent(
                            EcsEntity::parse(&entity)?,
                            EcsEntity::parse(&parent)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_unparent",
            |ctx: NativeCallContext<'_>, entity: ImmutableString| {
                call(ctx, EcsEntity::parse(&entity).map(EcsOperation::Unparent))
            },
        )
        .register_fn(
            "ecs_resource_insert",
            |ctx: NativeCallContext<'_>, name: ImmutableString, value: Dynamic| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::ResourceInsert(
                            type_name(name)?,
                            from_value::<Value>(value)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_resource_set",
            |ctx: NativeCallContext<'_>, name: ImmutableString, value: Dynamic| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::ResourceSet(
                            type_name(name)?,
                            from_value(value)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_resource_patch",
            |ctx: NativeCallContext<'_>, name: ImmutableString, fields: Map| {
                call(
                    ctx,
                    (|| {
                        Ok(EcsOperation::ResourcePatch(
                            type_name(name)?,
                            from_map(fields)?,
                        ))
                    })(),
                )
            },
        )
        .register_fn(
            "ecs_resource_remove",
            |ctx: NativeCallContext<'_>, name: ImmutableString| {
                call(ctx, type_name(name).map(EcsOperation::ResourceRemove))
            },
        );
}

fn call(ctx: NativeCallContext<'_>, operation: Result<EcsOperation, EcsError>) -> NativeResult {
    let result = (|| {
        let operation = operation.map_err(|error| ToolFailure::new(error.code, error.message))?;
        let run = ctx
            .tag()
            .and_then(|tag| tag.read_lock::<Run>())
            .ok_or_else(|| {
                ToolFailure::new("ecs_context_missing", "当前脚本没有 ECS 执行上下文")
            })?;
        let value = run
            .access
            .call(operation, &run.control.cancel, run.control.deadline)
            .map_err(|error| ToolFailure::new(error.code, error.message))?;
        rhai::serde::to_dynamic(value)
            .map_err(|error| ToolFailure::new("ecs_invalid_response", error.to_string()))
    })();
    result.map_err(|error| {
        EvalAltResult::ErrorRuntime(Dynamic::from(error), ctx.call_position()).into()
    })
}

fn from_map<T: DeserializeOwned>(map: Map) -> Result<T, EcsError> {
    from_value(Dynamic::from_map(map))
}

fn from_value<T: DeserializeOwned>(value: Dynamic) -> Result<T, EcsError> {
    let mut remaining = MAX_PAYLOAD_BYTES;
    validate_payload(&value, 0, &mut remaining)?;
    rhai::serde::from_dynamic(&value)
        .map_err(|error| EcsError::new("invalid_ecs_arguments", error.to_string()))
}

fn validate_payload(value: &Dynamic, depth: usize, remaining: &mut usize) -> Result<(), EcsError> {
    if depth > MAX_PAYLOAD_DEPTH {
        return Err(payload_limit());
    }
    reserve(remaining, VALUE_OVERHEAD)?;
    if let Some(value) = value.read_lock::<Map>() {
        for (key, value) in value.iter() {
            reserve(remaining, key.len())?;
            validate_payload(value, depth + 1, remaining)?;
        }
    } else if let Some(value) = value.read_lock::<Array>() {
        for value in value.iter() {
            validate_payload(value, depth + 1, remaining)?;
        }
    } else if let Some(value) = value.read_lock::<ImmutableString>() {
        reserve(remaining, value.len())?;
    } else if value
        .read_lock::<rhai::FLOAT>()
        .is_some_and(|number| !number.is_finite())
    {
        return Err(EcsError::new(
            "invalid_ecs_arguments",
            "ECS 参数须为有限浮点数",
        ));
    }
    Ok(())
}

fn reserve(remaining: &mut usize, bytes: usize) -> Result<(), EcsError> {
    *remaining = remaining.checked_sub(bytes).ok_or_else(payload_limit)?;
    Ok(())
}

fn payload_limit() -> EcsError {
    EcsError::new("ecs_input_limit", "ECS 参数超过大小或嵌套限制")
}

fn type_name(name: ImmutableString) -> Result<String, EcsError> {
    validate_name(&name)?;
    Ok(name.into())
}

fn validate_name(name: &str) -> Result<(), EcsError> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES {
        return Err(EcsError::new(
            "invalid_ecs_arguments",
            "类型名必须非空且长度不超过 1024 字节",
        ));
    }
    Ok(())
}

fn query(filter: Map) -> Result<Query, EcsError> {
    let query: Query = from_map(filter)?;
    if query.with.len() + query.without.len() > MAX_FILTER_TYPES {
        return Err(EcsError::new(
            "invalid_ecs_arguments",
            "查询至多支持 16 个组件过滤条件",
        ));
    }
    for name in query.with.iter().chain(&query.without).chain(&query.name) {
        validate_name(name)?;
    }
    Ok(query)
}
