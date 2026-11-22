use super::{ScriptResult, plain_data, with_run};
use crate::{ToolFailure, desktop::uia::GridAction};
use rhai::{Dynamic, Engine, INT, ImmutableString, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn(
            "uia_grid_info",
            |ctx: NativeCallContext, id: ImmutableString| grid(ctx, id, Ok(GridAction::Grid)),
        )
        .register_fn(
            "uia_grid_cell",
            |ctx: NativeCallContext, id: ImmutableString, row: INT, column: INT| {
                grid(ctx, id, GridAction::cell(row, column))
            },
        )
        .register_fn(
            "uia_grid_item_info",
            |ctx: NativeCallContext, id: ImmutableString| grid(ctx, id, Ok(GridAction::GridItem)),
        )
        .register_fn(
            "uia_table_info",
            |ctx: NativeCallContext, id: ImmutableString| grid(ctx, id, Ok(GridAction::Table)),
        )
        .register_fn(
            "uia_table_headers",
            |ctx: NativeCallContext, id: ImmutableString| grid(ctx, id, Ok(GridAction::Headers)),
        )
        .register_fn(
            "uia_table_item_info",
            |ctx: NativeCallContext, id: ImmutableString| grid(ctx, id, Ok(GridAction::TableItem)),
        )
        .register_fn(
            "uia_spreadsheet_info",
            |ctx: NativeCallContext, id: ImmutableString| {
                grid(ctx, id, Ok(GridAction::Spreadsheet(None)))
            },
        )
        .register_fn(
            "uia_spreadsheet_info",
            |ctx: NativeCallContext, id: ImmutableString, cell: ImmutableString| {
                grid(ctx, id, GridAction::named_cell(cell.into_owned()))
            },
        )
        .register_fn(
            "uia_spreadsheet_item_info",
            |ctx: NativeCallContext, id: ImmutableString| {
                grid(ctx, id, Ok(GridAction::SpreadsheetItem))
            },
        );
}

fn grid(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<GridAction, ToolFailure>,
) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
        run.uia
            .grid_pattern(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}
