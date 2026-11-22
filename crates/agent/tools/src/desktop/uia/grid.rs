use super::elements::{Element, parse_reference};
use super::patterns::{
    get_pattern, invalid_pattern_argument, optional_provider, pattern_result, provider,
};
use super::{RequestControl, Runtime, Worker};
use crate::ToolFailure;
use serde_json::{Value, json};
use std::time::Instant;
use tokio_util::sync::CancellationToken;
use uiautomation::{
    UIElement,
    patterns::{
        UIGridItemPattern, UIGridPattern, UISpreadsheetItemPattern, UISpreadsheetPattern,
        UITableItemPattern, UITablePattern, UIValuePattern,
    },
};
use uuid::Uuid;

const MAX_CELL_NAME: usize = 256;
const MAX_CELL_TEXT: usize = 4096;
const MAX_ANNOTATION_TYPES: usize = 64;

pub(crate) enum GridAction {
    Grid,
    Cell { row: i32, column: i32 },
    GridItem,
    Table,
    Headers,
    TableItem,
    Spreadsheet(Option<String>),
    SpreadsheetItem,
}

impl GridAction {
    pub(crate) fn cell(row: i64, column: i64) -> Result<Self, ToolFailure> {
        let index = |value| {
            i32::try_from(value)
                .ok()
                .filter(|value| *value >= 0)
                .ok_or_else(|| invalid_pattern_argument("行列索引须为可表示的非负整数"))
        };
        Ok(Self::Cell {
            row: index(row)?,
            column: index(column)?,
        })
    }

    pub(crate) fn named_cell(name: String) -> Result<Self, ToolFailure> {
        if name.is_empty() || name.chars().nth(MAX_CELL_NAME).is_some() {
            return Err(invalid_pattern_argument(
                "电子表格单元格名称为空或超出长度上限",
            ));
        }
        Ok(Self::Spreadsheet(Some(name)))
    }
}

impl Runtime {
    pub(crate) fn grid_pattern(
        &self,
        reference: &str,
        action: GridAction,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Value, ToolFailure> {
        let id = parse_reference(reference)?;
        self.call(cancel, deadline, move |worker, control| {
            worker.grid_pattern(id, action, control)
        })
    }
}

impl Worker {
    fn grid_pattern(
        &mut self,
        id: Uuid,
        action: GridAction,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let entry = self.resolve_element(id, control)?;
        let (pattern, operation, data) = match action {
            GridAction::Grid => {
                let pattern: UIGridPattern = get_pattern(&entry.element, control)?;
                let (rows, columns) = dimensions(&pattern, control)?;
                ("Grid", "info", json!({"rows": rows, "columns": columns}))
            }
            GridAction::Cell { row, column } => {
                let pattern: UIGridPattern = get_pattern(&entry.element, control)?;
                let (rows, columns) = dimensions(&pattern, control)?;
                if row >= rows || column >= columns {
                    return Err(invalid_pattern_argument(
                        "单元格坐标超出 Provider 报告的网格范围",
                    ));
                }
                let element = optional_provider(control, || pattern.get_item(row, column))?
                    .ok_or_else(cell_missing)?;
                let cell = self.one_grid_element(id, &entry, element, control)?;
                (
                    "Grid",
                    "cell",
                    json!({"row": row, "column": column, "cell": cell}),
                )
            }
            GridAction::GridItem => {
                let pattern: UIGridItemPattern = get_pattern(&entry.element, control)?;
                let row = provider(control, || pattern.get_row())?;
                let column = provider(control, || pattern.get_column())?;
                let row_span = provider(control, || pattern.get_row_span())?;
                let column_span = provider(control, || pattern.get_column_span())?;
                let owner = provider(control, || pattern.get_containing_grid())?;
                let grid = self.one_grid_element(id, &entry, owner, control)?;
                (
                    "GridItem",
                    "info",
                    json!({"row": row, "column": column,
                    "row_span": row_span, "column_span": column_span, "grid": grid}),
                )
            }
            GridAction::Table => {
                let pattern: UITablePattern = get_pattern(&entry.element, control)?;
                let major = provider(control, || pattern.get_row_or_column_major())?;
                let (rows, columns) = match get_pattern::<UIGridPattern>(&entry.element, control) {
                    Ok(grid) => {
                        let (r, c) = dimensions(&grid, control)?;
                        (Some(r), Some(c))
                    }
                    Err(error) if error.code == "uia_pattern_not_supported" => (None, None),
                    Err(error) => return Err(error),
                };
                (
                    "Table",
                    "info",
                    json!({"major": format!("{major:?}"), "rows": rows, "columns": columns}),
                )
            }
            GridAction::Headers => {
                let pattern: UITablePattern = get_pattern(&entry.element, control)?;
                let rows =
                    optional_provider(control, || pattern.get_row_headers())?.unwrap_or_default();
                let columns = optional_provider(control, || pattern.get_column_headers())?
                    .unwrap_or_default();
                (
                    "Table",
                    "headers",
                    self.grid_headers(id, &entry, rows, columns, control)?,
                )
            }
            GridAction::TableItem => {
                let pattern: UITableItemPattern = get_pattern(&entry.element, control)?;
                let rows = optional_provider(control, || pattern.get_row_header_items())?
                    .unwrap_or_default();
                let columns = optional_provider(control, || pattern.get_column_header_items())?
                    .unwrap_or_default();
                (
                    "TableItem",
                    "headers",
                    self.grid_headers(id, &entry, rows, columns, control)?,
                )
            }
            GridAction::Spreadsheet(name) => {
                let pattern: UISpreadsheetPattern = get_pattern(&entry.element, control)?;
                if let Some(name) = name {
                    let element = optional_provider(control, || pattern.get_item_by_name(&name))?
                        .ok_or_else(cell_missing)?;
                    let cell = self.one_grid_element(id, &entry, element, control)?;
                    (
                        "Spreadsheet",
                        "named_cell",
                        json!({"name": name, "cell": cell}),
                    )
                } else {
                    ("Spreadsheet", "info", json!({"can_find_named_cell": true}))
                }
            }
            GridAction::SpreadsheetItem => {
                let pattern: UISpreadsheetItemPattern = get_pattern(&entry.element, control)?;
                let formula = provider(control, || pattern.get_formula())?;
                let formula = bounded_cell_text(&formula);
                let value = match get_pattern::<UIValuePattern>(&entry.element, control) {
                    Ok(pattern) => Some(bounded_cell_text(&provider(control, || {
                        pattern.get_value()
                    })?)),
                    Err(error) if error.code == "uia_pattern_not_supported" => None,
                    Err(error) => return Err(error),
                };
                let kinds = optional_provider(control, || pattern.get_annotation_types())?
                    .unwrap_or_default();
                let objects = optional_provider(control, || pattern.get_annotation_objects())?
                    .unwrap_or_default();
                let kinds_truncated = kinds.len() > MAX_ANNOTATION_TYPES;
                let object_count = objects.len();
                let kinds = kinds
                    .into_iter()
                    .take(MAX_ANNOTATION_TYPES)
                    .collect::<Vec<_>>();
                let annotations = self.register_pattern_elements(id, &entry, objects, control)?;
                (
                    "SpreadsheetItem",
                    "info",
                    json!({"formula":formula, "value":value,
                    "annotation_types": kinds, "annotations":annotations,
                    "truncated": kinds_truncated || object_count > annotations.len()}),
                )
            }
        };
        Ok(pattern_result(id, pattern, operation, data))
    }

    fn one_grid_element(
        &mut self,
        id: Uuid,
        entry: &Element,
        element: UIElement,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        self.register_pattern_elements(id, entry, vec![element], control)?
            .pop()
            .ok_or_else(cell_missing)
    }

    fn grid_headers(
        &mut self,
        id: Uuid,
        entry: &Element,
        rows: Vec<UIElement>,
        columns: Vec<UIElement>,
        control: &RequestControl,
    ) -> Result<Value, ToolFailure> {
        let row_count = rows.len();
        let column_count = columns.len();
        let row_headers = self.register_pattern_elements(id, entry, rows, control)?;
        let column_headers = self.register_pattern_elements(id, entry, columns, control)?;
        let truncated = row_count > row_headers.len() || column_count > column_headers.len();
        Ok(
            json!({"row_headers": row_headers, "column_headers": column_headers,
            "row_count": row_count, "column_count": column_count, "truncated": truncated}),
        )
    }
}

fn dimensions(
    pattern: &UIGridPattern,
    control: &RequestControl,
) -> Result<(i32, i32), ToolFailure> {
    let rows = provider(control, || pattern.get_row_count())?;
    let columns = provider(control, || pattern.get_column_count())?;
    Ok((rows, columns))
}

fn bounded_cell_text(text: &str) -> Value {
    let mut chars = text.chars();
    let text: String = chars.by_ref().take(MAX_CELL_TEXT).collect();
    json!({"text": text, "truncated": chars.next().is_some()})
}

fn cell_missing() -> ToolFailure {
    ToolFailure::new("uia_element_not_found", "Provider 没有返回对应单元格")
}
