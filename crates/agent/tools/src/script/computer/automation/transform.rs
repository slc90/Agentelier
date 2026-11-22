use super::{ScriptResult, plain_data, with_uia};
use crate::{ToolFailure, desktop::uia::TransformAction};
use rhai::{Array, Dynamic, Engine, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn(
            "uia_transform_capabilities",
            |ctx: NativeCallContext, id: ImmutableString| {
                transform(ctx, id, Ok(TransformAction::Capabilities))
            },
        )
        .register_fn(
            "uia_transform",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString, args: Array| {
                let action =
                    rhai::serde::from_dynamic::<Vec<serde_json::Value>>(&Dynamic::from(args))
                        .map_err(|error| {
                            ToolFailure::new("uia_invalid_argument", error.to_string())
                        })
                        .and_then(|args| TransformAction::new(&action, &args));
                transform(ctx, id, action)
            },
        )
        .register_fn(
            "uia_dock_position",
            |ctx: NativeCallContext, id: ImmutableString| {
                transform(ctx, id, Ok(TransformAction::Dock(None)))
            },
        )
        .register_fn(
            "uia_dock_position",
            |ctx: NativeCallContext, id: ImmutableString, position: ImmutableString| {
                transform(ctx, id, TransformAction::dock_position(&position))
            },
        );
}

fn transform(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<TransformAction, ToolFailure>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .transform(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
