use super::Control;
use crate::{
    ToolFailure,
    ecs_access::{EcsAccess, EcsOperation},
};
use rhai::{Dynamic, Engine, EvalAltResult, NativeCallContext};

#[derive(Clone)]
pub(super) struct Run {
    pub(super) access: EcsAccess,
    pub(super) control: Control,
}

pub(super) fn register(engine: &mut Engine) {
    engine.register_fn("ecs_world_info", |ctx: NativeCallContext<'_>| {
        request(ctx, EcsOperation::WorldInfo)
    });
}

fn request(
    ctx: NativeCallContext<'_>,
    operation: EcsOperation,
) -> Result<Dynamic, Box<EvalAltResult>> {
    let result = (|| {
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
