use super::{
    discrete::{DiscreteChange, DiscreteValue},
    parameters,
    text_input::single_line,
};
use agentelier_agent_host::{
    AgentHostPlugin, EcsError, MountedScene, SceneDefinition, SceneState, SceneStateWrites,
    SceneStored,
};
use bevy::{
    prelude::*,
    text::{FontSize, Justify, LineBreak},
};
use bevy_widgetry::{
    scene::spawn_scene,
    table::{
        WidgetryTable, WidgetryTableAppExt, WidgetryTableCellRenderer, WidgetryTableCellValue,
        WidgetryTableColumn, WidgetryTableColumnWidth, WidgetryTableEvent, WidgetryTableEventKind,
        WidgetryTableHeaderRenderer, WidgetryTableHeaderValue, WidgetryTableLayout,
        WidgetryTableModel, WidgetryTableRowId, WidgetryTableSelection,
    },
    text::WidgetryText,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value as Json, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(super) const TEMPLATE: &str = "agentelier.data_table";
const SELECTED: &str = "selected_row";
const MAX_COLUMNS: usize = 12;
const MAX_ROWS: usize = 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    columns: Vec<Column>,
    rows: Vec<Row>,
    #[serde(default, skip_serializing_if = "Selection::omitted")]
    selected_row: Selection,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Column {
    id: String,
    label: String,
    kind: ColumnKind,
    unit: Option<String>,
    decimals: Option<u8>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ColumnKind {
    Text,
    Integer,
    Number,
    Boolean,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Row {
    id: String,
    values: Vec<Json>,
}

#[derive(Default)]
enum Selection {
    #[default]
    Omitted,
    Clear,
    Row(String),
}

impl Selection {
    fn omitted(&self) -> bool {
        matches!(self, Self::Omitted)
    }

    fn row(&self) -> Option<&str> {
        match self {
            Self::Row(id) => Some(id),
            Self::Omitted | Self::Clear => None,
        }
    }

    fn from_row(row: Option<String>) -> Self {
        row.map_or(Self::Clear, Self::Row)
    }
}

impl Serialize for Selection {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.row().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Selection {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Option::<String>::deserialize(deserializer).map(Self::from_row)
    }
}

#[derive(Reflect)]
struct CellDisplay {
    text: String,
    numeric: bool,
}

#[derive(Reflect)]
struct HeaderDisplay(String);

#[derive(Component)]
pub(super) struct DataTable {
    widget: Entity,
    source: Entity,
    columns: Vec<Column>,
    rows: BTreeMap<String, WidgetryTableRowId>,
    selected: Option<String>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Show a typed native data table with stable row identities and single row selection.",
        &[
            ("title", "Optional nonblank title."),
            ("columns", "Required 1 to 12 objects {id,label,kind,unit?,decimals?}. IDs are unique. kind is text, integer, number, or boolean. Numeric units appear in headers. decimals (0 to 6) formats number fields."),
            ("rows", "Required array of 0 to 1024 objects {id,values}. IDs are unique and stable across updates. Provide one typed value or null for each column in order."),
            (SELECTED, "Optional row ID. Omission on an update retains the current ID; null clears. A composition can use table_selection and table_field bindings."),
        ],
        build,
        None,
        Some(prepare_update),
    )?
    .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

pub(super) fn install(app: &mut App) -> Result {
    app.register_widgetry_table::<Row>()
        .register_table_cell_renderer(WidgetryTableCellRenderer::new(render_cell))
        .map_err(|error| error.into_bevy_error())?
        .register_table_header_renderer(WidgetryTableHeaderRenderer::new(render_header))
        .map_err(|error| error.into_bevy_error())?
        .add_observer(on_widgetry_selection);
    Ok(())
}

fn render_cell(value: &CellDisplay) -> impl SceneList + use<> {
    let text = value.text.clone();
    let justify = if value.numeric {
        Justify::Right
    } else {
        Justify::Left
    };
    bsn_list! {
        Text(text) WidgetryText TextLayout { justify, linebreak: LineBreak::WordOrCharacter }
        Node { width: percent(100), min_width: px(0), overflow: Overflow::clip() }
    }
}

fn render_header(label: &HeaderDisplay) -> impl SceneList + use<> {
    let text = label.0.clone();
    bsn_list! {
        Text(text) WidgetryText TextLayout::linebreak(LineBreak::WordOrCharacter)
        Node { width: percent(100), min_width: px(0), overflow: Overflow::clip() }
    }
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    props.validate().map_err(invalid)?;
    let root = spawn_scene(world, bsn! {
        ChildOf(parent) Name("NativeDataTable")
        Node { width: percent(100), min_width: px(0), max_width: px(1000), flex_direction: FlexDirection::Column, row_gap: px(6) }
    }).map_err(construction)?;
    if let Err(error) = decorate(world, root, props) {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

fn decorate(
    world: &mut World,
    root: Entity,
    props: Properties,
) -> std::result::Result<(), EcsError> {
    let Properties {
        title,
        columns,
        rows,
        selected_row,
    } = props;
    let mut model = WidgetryTableModel::<Row>::default();
    let mut layout = WidgetryTableLayout::default();
    layout.row_height = 40.0;
    layout.column_header_height = 42.0;
    layout.row_header_width = 36.0;
    for (index, column) in columns.iter().enumerate() {
        let id = model
            .push_column(WidgetryTableColumn::new(
                WidgetryTableHeaderValue::new(HeaderDisplay(column.label_with_unit())),
                (index, column.clone()),
                |row: &Row, (index, column)| {
                    WidgetryTableCellValue::new(CellDisplay {
                        text: column.format(&row.values[*index]),
                        numeric: column.numeric(),
                    })
                },
            ))
            .map_err(construction)?;
        let width = if matches!(column.kind, ColumnKind::Text) {
            192.0
        } else {
            132.0
        };
        layout = layout.with_column_width(id, WidgetryTableColumnWidth::Fixed(width));
    }
    let height = 42.0 + 40.0 * (rows.len().clamp(1, 7) as f32);
    let mut index = BTreeMap::new();
    for row in rows {
        let id = row.id.clone();
        index.insert(id, model.push_row(row).map_err(construction)?);
    }
    let source = world
        .spawn((ChildOf(root), Name::new("DataTableSource"), model))
        .id();
    if let Some(title) = title {
        spawn_scene(
            world,
            bsn! {
                ChildOf(root) Text(title) WidgetryText TextFont { font_size: FontSize::Px(19.0) }
                TextLayout::linebreak(LineBreak::WordOrCharacter)
            },
        )
        .map_err(construction)?;
    }
    let widget = spawn_scene(
        world,
        bsn! {
            ChildOf(root) Name("DataTableWidget")
            @WidgetryTable::<Row> { @source: source, @layout: {layout.clone()} }
            Node { width: percent(100), min_width: px(0), height: px(height) }
        },
    )
    .map_err(construction)?;
    world.entity_mut(root).insert(DataTable {
        widget,
        source,
        columns,
        rows: index,
        selected: None,
    });
    set_selection(world, root, selected_row.row()).map_err(construction)?;
    Ok(())
}

impl Properties {
    fn validate(&self) -> Result {
        if self
            .title
            .as_ref()
            .is_some_and(|title| !valid_label(title, 256))
        {
            return Err(io::Error::other("title 必须是非空单行文字，最多 256 字节").into());
        }
        let mut ids = BTreeSet::new();
        if !(1..=MAX_COLUMNS).contains(&self.columns.len())
            || self.columns.iter().any(|column| {
                !valid_label(&column.id, 64)
                    || !valid_label(&column.label, 96)
                    || !ids.insert(column.id.as_str())
                    || column
                        .unit
                        .as_ref()
                        .is_some_and(|unit| !column.numeric() || !valid_label(unit, 32))
                    || column.decimals.is_some_and(|decimals| {
                        !matches!(column.kind, ColumnKind::Number) || decimals > 6
                    })
            })
        {
            return Err(io::Error::other("columns 需要 1 至 12 个有效且 ID 唯一的列").into());
        }
        let mut ids = BTreeSet::new();
        if self.rows.len() > MAX_ROWS
            || self.rows.iter().any(|row| {
                !valid_label(&row.id, 64)
                    || !ids.insert(row.id.as_str())
                    || row.values.len() != self.columns.len()
                    || row
                        .values
                        .iter()
                        .zip(&self.columns)
                        .any(|(value, column)| !column.accepts(value))
            })
        {
            return Err(
                io::Error::other("数据行的 ID 必须唯一，单元数量和类型必须匹配列定义").into(),
            );
        }
        self.validate_selection()
    }

    fn validate_selection(&self) -> Result {
        if self
            .selected_row
            .row()
            .is_some_and(|id| !self.rows.iter().any(|row| row.id == id))
        {
            return Err(io::Error::other(
                "selected_row 需要有效的行 ID，移除已选行时请同时指定新 ID 或 null",
            )
            .into());
        }
        Ok(())
    }
}

impl Column {
    fn numeric(&self) -> bool {
        matches!(self.kind, ColumnKind::Integer | ColumnKind::Number)
    }

    fn accepts(&self, value: &Json) -> bool {
        value.is_null()
            || match self.kind {
                ColumnKind::Text => value
                    .as_str()
                    .is_some_and(|text| text.len() <= 1024 && text.chars().all(single_line)),
                ColumnKind::Integer => value.as_i64().is_some(),
                ColumnKind::Number => value.as_f64().is_some_and(f64::is_finite),
                ColumnKind::Boolean => value.is_boolean(),
            }
    }

    fn label_with_unit(&self) -> String {
        match &self.unit {
            Some(unit) => format!("{} ({unit})", self.label),
            None => self.label.clone(),
        }
    }

    fn format(&self, value: &Json) -> String {
        match value {
            Json::Null => "—".into(),
            Json::String(value) => value.clone(),
            Json::Bool(value) => {
                if *value {
                    "是".into()
                } else {
                    "否".into()
                }
            }
            Json::Number(value) => match (value.as_f64(), self.decimals) {
                (Some(value), Some(decimals)) => {
                    format!("{value:.precision$}", precision = usize::from(decimals))
                }
                _ => value.to_string(),
            },
            _ => "—".into(),
        }
    }
}

fn prepare_update(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if props.selected_row.omitted() {
        props.selected_row = Selection::from_row(table(world, root)?.selected.clone());
    }
    props.validate_selection()?;
    Ok(props)
}

fn restore(mut props: Properties, state: &SceneState) -> Result<Properties> {
    props.selected_row = saved_selection(&state.values)?;
    props.validate_selection()?;
    Ok(props)
}

fn prepare_unmounted(mut props: Properties, archived: &SceneStored) -> Result<Properties> {
    if props.selected_row.omitted() {
        props.selected_row = if let Some(values) = &archived.values {
            saved_selection(values)?
        } else {
            properties(&archived.definition)?.selected_row
        };
    }
    props.validate_selection()?;
    Ok(props)
}

fn saved_selection(values: &Map<String, Json>) -> Result<Selection> {
    let (Some(value), 1) = (values.get(SELECTED), values.len()) else {
        return Err(io::Error::other("保存的表格状态缺少选中行").into());
    };
    Ok(serde_json::from_value(value.clone())?)
}

pub(super) fn set_selection(
    world: &mut World,
    root: Entity,
    selection: Option<&str>,
) -> Result<bool> {
    let table = table(world, root)?;
    let native = match selection {
        Some(id) => WidgetryTableSelection::Row(
            *table
                .rows
                .get(id)
                .ok_or_else(|| io::Error::other("表格缺少要选择的行"))?,
        ),
        None => WidgetryTableSelection::None,
    };
    let widget = table.widget;
    let previous = table.selected.clone();
    let changed = previous.as_deref() != selection;
    if changed {
        world
            .get_mut::<DataTable>(root)
            .ok_or_else(|| io::Error::other("表格缺少选择状态"))?
            .selected = selection.map(str::to_owned);
    }
    if let Err(error) = WidgetryTable::<Row>::set_selection(world, widget, native) {
        if changed && let Some(mut table) = world.get_mut::<DataTable>(root) {
            table.selected = previous;
        }
        return Err(error.into_bevy_error());
    }
    Ok(changed)
}

fn on_widgetry_selection(event: On<WidgetryTableEvent>, mut commands: Commands) {
    let widget = event.entity;
    let kind = event.kind;
    match kind {
        WidgetryTableEventKind::RowSelected(_)
        | WidgetryTableEventKind::CellSelected { .. }
        | WidgetryTableEventKind::SelectionCleared
        | WidgetryTableEventKind::ColumnSelected(_)
        | WidgetryTableEventKind::RangeSelected(_) => {
            commands.queue(move |world: &mut World| -> Result {
                let Some(root) = world.get::<ChildOf>(widget).map(ChildOf::parent) else {
                    return Ok(());
                };
                let Some(table) = world
                    .get::<DataTable>(root)
                    .filter(|table| table.widget == widget)
                else {
                    return Ok(());
                };
                let selected = match kind {
                    WidgetryTableEventKind::RowSelected(row)
                    | WidgetryTableEventKind::CellSelected { row, .. } => {
                        let model = world
                            .get::<WidgetryTableModel<Row>>(table.source)
                            .ok_or_else(|| io::Error::other("原生表格缺少数据源"))?;
                        let Some(index) = model.row_index(row) else {
                            return Ok(());
                        };
                        model.row(index).map(|row| row.id.clone())
                    }
                    WidgetryTableEventKind::SelectionCleared
                        if !parameters::controls_table(world, root) =>
                    {
                        None
                    }
                    _ => table.selected.clone(),
                };
                if set_selection(world, root, selected.as_deref())? {
                    if let Some(&mount) = world.get::<MountedScene>(root) {
                        let state = SceneState {
                            revision: mount.revision,
                            values: Map::from_iter([(SELECTED.into(), json!(selected))]),
                        };
                        world
                            .resource_mut::<SceneStateWrites>()
                            .schedule(root, mount, state);
                    }
                    world.trigger(DiscreteChange {
                        source: root,
                        value: DiscreteValue::Choice(selected),
                        is_final: true,
                    });
                }
                Ok(())
            })
        }
        _ => {}
    }
}

pub(super) fn validate_binding<'a>(
    scene: &SceneDefinition,
    ids: impl IntoIterator<Item = &'a str>,
) -> Result {
    let props = properties(scene)?;
    let rows: BTreeSet<_> = props.rows.iter().map(|row| row.id.as_str()).collect();
    let choices: Vec<_> = ids.into_iter().collect();
    let unique: BTreeSet<_> = choices.iter().copied().collect();
    if rows != unique || unique.len() != choices.len() {
        return Err(io::Error::other("表格选择值需要与可选择的行 ID 逐一对应").into());
    }
    props.validate()
}

pub(super) fn initial_field(scene: &SceneDefinition, field: &str) -> Result<String> {
    let props = properties(scene)?;
    let row = props
        .selected_row
        .row()
        .and_then(|id| props.rows.iter().find(|row| row.id == id))
        .ok_or_else(|| io::Error::other("表格字段绑定需要已选中的有效行"))?;
    format_field(&props.columns, row, field)
}

pub(super) fn current_field(world: &World, root: Entity, row: &str, field: &str) -> Result<String> {
    let table = table(world, root)?;
    let model = world
        .get::<WidgetryTableModel<Row>>(table.source)
        .ok_or_else(|| io::Error::other("表格缺少数据源"))?;
    let row = table
        .rows
        .get(row)
        .and_then(|&id| model.row_index(id))
        .and_then(|index| model.row(index))
        .ok_or_else(|| io::Error::other("表格字段缺少有效的选择行"))?;
    format_field(&table.columns, row, field)
}

fn format_field(columns: &[Column], row: &Row, field: &str) -> Result<String> {
    let (index, column) = columns
        .iter()
        .enumerate()
        .find(|(_, column)| column.id == field)
        .ok_or_else(|| io::Error::other("表格缺少指定列 ID"))?;
    Ok(format!(
        "{} · {}",
        column.label_with_unit(),
        column.format(&row.values[index])
    ))
}

fn table(world: &World, root: Entity) -> Result<&DataTable> {
    world
        .get::<DataTable>(root)
        .ok_or_else(|| io::Error::other("原生表格缺少数据状态").into())
}

fn properties(scene: &SceneDefinition) -> Result<Properties> {
    Ok(serde_json::from_value(Json::Object(scene.props.clone()))?)
}

fn valid_label(label: &str, max: usize) -> bool {
    !label.trim().is_empty() && label.len() <= max && label.chars().all(single_line)
}

fn invalid(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("invalid_scene_props", error.to_string())
}
fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
