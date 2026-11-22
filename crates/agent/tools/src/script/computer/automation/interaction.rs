use super::{ScriptResult, plain_data, with_uia};
use crate::{ToolFailure, desktop::uia::InteractionAction};
use rhai::{Engine, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn(
            "uia_drag_info",
            |ctx: NativeCallContext, id: ImmutableString| {
                interact(ctx, id, Ok(InteractionAction::Drag))
            },
        )
        .register_fn(
            "uia_drop_target_info",
            |ctx: NativeCallContext, id: ImmutableString| {
                interact(ctx, id, Ok(InteractionAction::DropTarget))
            },
        )
        .register_fn(
            "uia_custom_navigation",
            |ctx: NativeCallContext, id: ImmutableString, direction: ImmutableString| {
                interact(ctx, id, InteractionAction::navigate(&direction))
            },
        )
        .register_fn(
            "uia_synchronized_input",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                interact(ctx, id, InteractionAction::synchronized(&action, None))
            },
        )
        .register_fn(
            "uia_synchronized_input",
            |ctx: NativeCallContext,
             id: ImmutableString,
             action: ImmutableString,
             input: ImmutableString| {
                interact(
                    ctx,
                    id,
                    InteractionAction::synchronized(&action, Some(&input)),
                )
            },
        );
}

fn interact(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<InteractionAction, ToolFailure>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .interact(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
