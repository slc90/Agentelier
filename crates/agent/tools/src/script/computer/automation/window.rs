use super::{ScriptResult, plain_data, with_uia};
use crate::{ToolFailure, desktop::uia::WindowAction};
use rhai::{Engine, INT, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn(
            "uia_window_state",
            |ctx: NativeCallContext, id: ImmutableString| window(ctx, id, Ok(WindowAction::State)),
        )
        .register_fn(
            "uia_window_action",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                window(ctx, id, WindowAction::from_name(&action))
            },
        )
        .register_fn(
            "uia_window_action",
            |ctx: NativeCallContext,
             id: ImmutableString,
             action: ImmutableString,
             timeout_ms: INT| {
                let action = if action == "wait_for_input_idle" {
                    WindowAction::wait_idle(timeout_ms)
                } else {
                    Err(ToolFailure::new(
                        "uia_invalid_argument",
                        "只有 wait_for_input_idle 接受等待时长",
                    ))
                };
                window(ctx, id, action)
            },
        );
}

fn window(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<WindowAction, ToolFailure>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .window_pattern(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
