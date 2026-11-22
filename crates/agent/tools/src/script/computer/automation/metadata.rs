use super::{ScriptResult, plain_data, with_run};
use crate::{
    ToolFailure,
    desktop::uia::{LegacyAction, MetadataAction},
};
use rhai::{Dynamic, Engine, INT, ImmutableString, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_object_model", object_model)
        .register_fn(
            "uia_legacy_accessible",
            |ctx: NativeCallContext, id: ImmutableString| {
                metadata(ctx, id, Ok(MetadataAction::Legacy(LegacyAction::Info)))
            },
        )
        .register_fn(
            "uia_legacy_accessible",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                metadata(
                    ctx,
                    id,
                    if action == "default_action" {
                        Ok(MetadataAction::Legacy(LegacyAction::DefaultAction))
                    } else {
                        Err(invalid_action())
                    },
                )
            },
        )
        .register_fn(
            "uia_legacy_accessible",
            |ctx: NativeCallContext,
             id: ImmutableString,
             action: ImmutableString,
             value: ImmutableString| {
                metadata(
                    ctx,
                    id,
                    if action == "set_value" {
                        Ok(MetadataAction::Legacy(LegacyAction::SetValue(
                            value.into_owned(),
                        )))
                    } else {
                        Err(invalid_action())
                    },
                )
            },
        )
        .register_fn(
            "uia_legacy_accessible",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString, flags: INT| {
                metadata(
                    ctx,
                    id,
                    if action == "select" {
                        MetadataAction::select(flags)
                    } else {
                        Err(invalid_action())
                    },
                )
            },
        )
        .register_fn(
            "uia_multiple_view",
            |ctx: NativeCallContext, id: ImmutableString| {
                metadata(ctx, id, Ok(MetadataAction::Views(None)))
            },
        )
        .register_fn(
            "uia_multiple_view",
            |ctx: NativeCallContext, id: ImmutableString, view: INT| {
                metadata(ctx, id, MetadataAction::set_view(view))
            },
        )
        .register_fn(
            "uia_annotation",
            |ctx: NativeCallContext, id: ImmutableString| {
                metadata(ctx, id, Ok(MetadataAction::Annotation))
            },
        )
        .register_fn(
            "uia_styles",
            |ctx: NativeCallContext, id: ImmutableString| {
                metadata(ctx, id, Ok(MetadataAction::Styles))
            },
        );
}

fn object_model(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        run.uia
            .object_model(&id, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn metadata(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<MetadataAction, ToolFailure>,
) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        run.uia
            .metadata_pattern(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn invalid_action() -> ToolFailure {
    ToolFailure::new(
        "uia_invalid_argument",
        "旧式可访问操作须为 default_action、set_value 或 select，且提供对应参数",
    )
}
