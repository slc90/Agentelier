use super::{ScriptResult, plain_data, strings, with_run};
use crate::desktop::uia::SnapshotRequest;
use rhai::{Array, Dynamic, Engine, INT, ImmutableString, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_snapshot", create)
        .register_fn("uia_snapshot_refresh", refresh)
        .register_fn("uia_snapshot_status", status);
}

fn create(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    view: ImmutableString,
    properties: Array,
    max_nodes: INT,
) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        let request = SnapshotRequest::new(&reference, &view, &strings(properties)?, max_nodes)?;
        run.uia
            .snapshot(request, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn refresh(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    scope: ImmutableString,
) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        run.uia
            .snapshot_refresh(&id, &scope, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn status(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        run.uia
            .snapshot_status(&id, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
