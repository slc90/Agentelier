use super::{ScriptResult, fields, with_run};
use crate::desktop::uia::RootSummary;
use rhai::{Dynamic, Engine, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_status", status)
        .register_fn("uia_desktop_root", desktop_root)
        .register_fn("uia_window_root", window_root)
        .register_fn("uia_root_info", root_info);
}

fn status(ctx: NativeCallContext<'_>) -> ScriptResult<bool> {
    with_run(ctx, |run| {
        run.uia.status(&run.control.cancel, run.control.deadline)
    })
}

fn desktop_root(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.uia
            .desktop_root(&run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn window_root(ctx: NativeCallContext<'_>, window_id: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.uia
            .window_root(&window_id, &run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn root_info(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.uia
            .root_info(&reference, &run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn root_fields(summary: RootSummary) -> Map {
    fields([
        ("ref", summary.reference.into()),
        (
            "window_id",
            summary.window_id.map_or(Dynamic::UNIT, Dynamic::from),
        ),
        ("name", summary.name.into()),
        ("class_name", summary.class_name.into()),
        ("control_type", summary.control_type.into()),
        ("provider_accessible", true.into()),
    ])
}
