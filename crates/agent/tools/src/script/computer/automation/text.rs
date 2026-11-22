use super::{ScriptResult, plain_data, with_uia};
use crate::{
    ToolFailure,
    desktop::uia::{SelectionChange, TextAction, TextRangeAction},
};
use rhai::{Engine, INT, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn(
            "uia_text_document_range",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Document),
        )
        .register_fn(
            "uia_text_selection",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Selection),
        )
        .register_fn(
            "uia_text_visible_ranges",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Visible),
        )
        .register_fn(
            "uia_text_caret",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Caret),
        )
        .register_fn(
            "uia_text_annotation_range",
            |ctx: NativeCallContext, id: ImmutableString, annotation: ImmutableString| {
                text(ctx, id, TextAction::Annotation(annotation.into_owned()))
            },
        )
        .register_fn(
            "uia_text_edit_state",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Edit),
        )
        .register_fn(
            "uia_text_child_range",
            |ctx: NativeCallContext, id: ImmutableString| text(ctx, id, TextAction::Child),
        )
        .register_fn(
            "uia_text_get",
            |ctx: NativeCallContext, id: ImmutableString, budget: INT| {
                range(ctx, id, TextRangeAction::read(budget))
            },
        )
        .register_fn(
            "uia_text_find",
            |ctx: NativeCallContext,
             id: ImmutableString,
             query: ImmutableString,
             backward: bool,
             ignore_case: bool| {
                range(
                    ctx,
                    id,
                    TextRangeAction::find(query.into_owned(), backward, ignore_case),
                )
            },
        )
        .register_fn(
            "uia_text_range_move",
            |ctx: NativeCallContext, id: ImmutableString, unit: ImmutableString, count: INT| {
                range(ctx, id, TextRangeAction::movement(&unit, count, None))
            },
        )
        .register_fn(
            "uia_text_range_move",
            |ctx: NativeCallContext,
             id: ImmutableString,
             unit: ImmutableString,
             count: INT,
             endpoint: ImmutableString| {
                range(
                    ctx,
                    id,
                    TextRangeAction::movement(&unit, count, Some(&endpoint)),
                )
            },
        )
        .register_fn(
            "uia_text_range_expand",
            |ctx: NativeCallContext, id: ImmutableString, unit: ImmutableString| {
                range(ctx, id, TextRangeAction::expand(&unit))
            },
        )
        .register_fn(
            "uia_text_range_select",
            |ctx: NativeCallContext, id: ImmutableString| {
                range(
                    ctx,
                    id,
                    Ok(TextRangeAction::Select(SelectionChange::Select)),
                )
            },
        )
        .register_fn(
            "uia_text_range_select",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                range(ctx, id, action.parse().map(TextRangeAction::Select))
            },
        )
        .register_fn(
            "uia_text_range_scroll",
            |ctx: NativeCallContext, id: ImmutableString, align_to_top: bool| {
                range(ctx, id, Ok(TextRangeAction::Scroll(align_to_top)))
            },
        )
        .register_fn(
            "uia_text_range_attribute",
            |ctx: NativeCallContext, id: ImmutableString, name: ImmutableString| {
                range(ctx, id, TextRangeAction::attribute(&name))
            },
        );
}

fn text(ctx: NativeCallContext<'_>, id: ImmutableString, action: TextAction) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .text_action(&id, action, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn range(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<TextRangeAction, ToolFailure>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .text_range_action(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
