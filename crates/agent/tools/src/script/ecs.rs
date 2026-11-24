use super::Control;
use crate::{
    ToolFailure,
    ecs_access::{EcsAccess, EcsEntity, EcsError, EcsOperation, Page, Query},
};
use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map, NativeCallContext};
use serde::de::DeserializeOwned;

const MAX_NAME_BYTES: usize = 1024;
const MAX_FILTER_TYPES: usize = 16;
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
    rhai::serde::from_dynamic(&Dynamic::from_map(map))
        .map_err(|error| EcsError::new("invalid_ecs_arguments", error.to_string()))
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
