use super::{ScriptResult, plain_data, strings, with_uia};
use crate::{
    ToolFailure,
    desktop::uia::{EventRequest, WaitRequest},
};
use rhai::{Array, Dynamic, Engine, INT, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_subscribe", subscribe)
        .register_fn("uia_unsubscribe", unsubscribe)
        .register_fn("uia_events_next", next)
        .register_fn("uia_wait", wait);
}

fn subscribe(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    event: ImmutableString,
    scope: ImmutableString,
    properties: Array,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let request = EventRequest::new(&event, &scope, &strings(properties)?)?;
        run.uia
            .subscribe(
                &reference,
                request,
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
    })
}

fn unsubscribe(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .unsubscribe(&id, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn next(ctx: NativeCallContext<'_>, timeout_ms: INT, max_events: INT) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .events_next(
                timeout_ms,
                max_events,
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
    })
}

fn wait(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    condition: Dynamic,
    timeout_ms: INT,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let condition = rhai::serde::from_dynamic(&condition)
            .map_err(|error| ToolFailure::new("uia_invalid_argument", error.to_string()))?;
        let request = WaitRequest::new(&reference, condition, timeout_ms)?;
        run.uia
            .wait(request, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
