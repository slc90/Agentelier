use super::{Reply, ScriptResult, Source, strings, with_reply};
use crate::desktop::uia::SnapshotRequest;
use rhai::{Array, Engine, INT, ImmutableString, Map, NativeCallContext};

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
) -> ScriptResult<Map> {
    with_reply(ctx, Source::Snapshot, |run| {
        let request = SnapshotRequest::new(&reference, &view, &strings(properties)?, max_nodes)?;
        run.uia
            .snapshot(request, &run.control.cancel, run.control.deadline)
            .and_then(Reply::snapshot)
    })
}

fn refresh(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    scope: ImmutableString,
) -> ScriptResult<Map> {
    with_reply(ctx, Source::Snapshot, |run| {
        run.uia
            .snapshot_refresh(&id, &scope, &run.control.cancel, run.control.deadline)
            .and_then(Reply::snapshot)
    })
}

fn status(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_reply(ctx, Source::Snapshot, |run| {
        run.uia
            .snapshot_status(&id, &run.control.cancel, run.control.deadline)
            .and_then(Reply::snapshot)
    })
}
