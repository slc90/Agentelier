use super::{
    construction, invalid,
    plot::{self, display_number},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::{
    prelude::*,
    text::{FontSize, Justify, LineBreak, LineHeight},
    ui::{ColorStop, InColorSpace, LinearGradient},
};
use bevy_widgetry::{
    scene::spawn_scene,
    scroll_area::{ScrollAxis, WidgetryScrollArea},
    text::{WidgetryText, WidgetryTextColorOverrides, WidgetryTextStateColorOverrides},
    theme::WidgetryTheme,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.heatmap";
const MAX_AXIS: usize = 8;
const ROW_LABEL_WIDTH: f32 = 138.0;
const CELL_WIDTH: f32 = 94.0;
const CELL_HEIGHT: f32 = 44.0;
const GAP: f32 = 3.0;
const SCROLLBAR: f32 = 14.0;
const LOW: Color = Color::srgb_u8(50, 84, 146);
const MID: Color = Color::srgb_u8(238, 225, 199);
const HIGH: Color = Color::srgb_u8(154, 51, 50);
const MISSING: Color = Color::srgb_u8(183, 190, 185);

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    rows: Vec<String>,
    columns: Vec<String>,
    values: Vec<Vec<Option<f64>>>,
    unit: Option<String>,
    color_range: Option<[f64; 2]>,
    selected_row: Option<String>,
}

#[derive(Clone, Copy)]
enum ColorScale {
    Range { minimum: f64, maximum: f64 },
    Constant(f64),
    Empty,
}

#[derive(Component, Default, Clone)]
struct Grid;

#[derive(Component, Default, Clone, Copy)]
struct DataCell(usize);

#[derive(Component)]
pub(in crate::agent_scenes) struct Heatmap {
    rows: Vec<String>,
    cells: Vec<(usize, Entity)>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Show a numeric matrix as a native heatmap with a labeled color scale and explicit missing cells.",
        &[
            ("title", "Optional nonblank title."),
            ("rows", "Required 1 to 8 unique row labels in top-to-bottom order."),
            ("columns", "Required 1 to 8 unique column labels in left-to-right order."),
            ("values", "One array per row, containing one finite number or null per column. Null uses a dedicated missing color."),
            ("unit", "Optional unit for the numeric color scale."),
            ("color_range", "Optional [minimum, maximum], both finite and increasing. Values outside it use the nearest color endpoint. If omitted, uses the full observed data."),
            ("selected_row", "Optional row name. Null or omitted shows all rows. A composition can use category_filter."),
        ],
        build,
        None,
        None,
    )
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    props.validate()?;
    let scale = ColorScale::from_properties(&props)?;
    let root = plot::root(world, parent, "NativeHeatmap", props.title.as_deref())?;
    if let Err(error) = decorate(world, root, props, scale) {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

impl Properties {
    fn validate(&self) -> std::result::Result<(), EcsError> {
        if !valid_axis(&self.rows) || !valid_axis(&self.columns) {
            return Err(invalid(
                "行和列各需 1 至 8 个唯一的非空单行标签，每个最多 48 字节",
            ));
        }
        if self.values.len() != self.rows.len()
            || self
                .values
                .iter()
                .any(|row| row.len() != self.columns.len())
        {
            return Err(invalid("values 的矩阵尺寸必须与行列标签逐一对应"));
        }
        if self
            .values
            .iter()
            .flatten()
            .flatten()
            .any(|value| !value.is_finite())
        {
            return Err(invalid("单元值必须有限，缺失用 null"));
        }
        if self
            .unit
            .as_ref()
            .is_some_and(|unit| !valid_label(unit, 32))
        {
            return Err(invalid("unit 必须是非空单行文本，最多 32 字节"));
        }
        if self
            .selected_row
            .as_ref()
            .is_some_and(|row| !self.rows.contains(row))
        {
            return Err(invalid("selected_row 必须对应行标签"));
        }
        Ok(())
    }
}

impl ColorScale {
    fn from_properties(props: &Properties) -> std::result::Result<Self, EcsError> {
        if let Some([minimum, maximum]) = props.color_range {
            if !minimum.is_finite() || !maximum.is_finite() || minimum >= maximum {
                return Err(invalid("color_range 必须有限且从小到大"));
            }
            return Ok(Self::Range { minimum, maximum });
        }
        let mut values = props.values.iter().flatten().flatten().copied();
        let Some(first) = values.next() else {
            return Ok(Self::Empty);
        };
        let (minimum, maximum) = values.fold((first, first), |(min, max), value| {
            (min.min(value), max.max(value))
        });
        Ok(if minimum == maximum {
            Self::Constant(minimum)
        } else {
            Self::Range { minimum, maximum }
        })
    }

    fn color(self, value: Option<f64>) -> Color {
        let Some(value) = value else { return MISSING };
        match self {
            Self::Range { minimum, maximum } => {
                let value = value.clamp(minimum, maximum);
                let span = maximum - minimum;
                let fraction = if span.is_finite() {
                    (value - minimum) / span
                } else {
                    (value * 0.5 - minimum * 0.5) / (maximum * 0.5 - minimum * 0.5)
                };
                ramp(fraction.clamp(0.0, 1.0) as f32)
            }
            Self::Constant(_) | Self::Empty => MID,
        }
    }

    fn legend(self, unit: Option<&str>) -> Box<dyn Scene> {
        match self {
            Self::Range { minimum, maximum } => {
                let title = unit.map_or_else(|| "色标".into(), |unit| format!("色标 ({unit})"));
                let low = display_number(minimum);
                let midpoint = display_number(minimum.midpoint(maximum));
                let high = display_number(maximum);
                let gradient = BackgroundGradient::from(
                    LinearGradient::to_right(vec![
                        ColorStop::percent(LOW, 0.0),
                        ColorStop::percent(MID, 50.0),
                        ColorStop::percent(HIGH, 100.0),
                    ])
                    .in_srgb(),
                );
                Box::new(bsn! {
                    Name("HeatmapScale") Node { width: percent(100), max_width: px(430), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(3) }
                    Children [
                        Text(title) WidgetryText--
                        template(move |_| Ok(gradient.clone())) Node { width: percent(100), height: px(12), border_radius: BorderRadius::all(px(3)) }--
                        Node { min_width: px(0), width: percent(100), justify_content: JustifyContent::SpaceBetween, column_gap: px(5), flex_wrap: FlexWrap::Wrap }
                        Children [Text(low) WidgetryText-- Text(midpoint) WidgetryText-- Text(high) WidgetryText]
                    ]
                })
            }
            Self::Constant(value) => {
                let label = unit.map_or_else(
                    || format!("有效值均为 {}", display_number(value)),
                    |unit| format!("有效值均为 {} {unit}", display_number(value)),
                );
                Box::new(
                    bsn! { Name("HeatmapScale") @plot::legend(plot::Legend { name: label, color: MID }, 14.0, 14.0) },
                )
            }
            Self::Empty => Box::new(bsn! { Name("HeatmapScale") Text("无有效值") WidgetryText }),
        }
    }
}

fn decorate(
    world: &mut World,
    root: Entity,
    props: Properties,
    scale: ColorScale,
) -> std::result::Result<(), EcsError> {
    let theme = world.resource::<WidgetryTheme>();
    let ink = theme.text.normal.foreground;
    let header = theme.text_field.editable.normal.background.with_alpha(1.0);
    let mut cells = vec![label_cell("", header, ink)];
    cells.extend(
        props
            .columns
            .iter()
            .map(|label| label_cell(label, header, ink)),
    );
    for (row, (label, values)) in props.rows.iter().zip(&props.values).enumerate() {
        cells.push(label_cell(label, header, ink));
        for (column, value) in values.iter().copied().enumerate() {
            let background = scale.color(value);
            let text = value.map_or_else(|| "—".into(), display_number);
            let color = plot::contrast(background);
            let text_scene = cell(text, background, color);
            let name = format!("HeatmapCell-{row}-{column}");
            cells.push(Box::new(
                bsn! { @text_scene Name(name) DataCell(row) Visibility::Inherited },
            ));
        }
    }
    let columns = vec![
        RepeatedGridTrack::px(1, ROW_LABEL_WIDTH),
        RepeatedGridTrack::px(props.columns.len() as u16, CELL_WIDTH),
    ];
    let width = ROW_LABEL_WIDTH + (CELL_WIDTH + GAP) * props.columns.len() as f32;
    let height = CELL_HEIGHT * (props.rows.len() + 1) as f32 + GAP * props.rows.len() as f32;
    let scroll_height = height + SCROLLBAR;
    let title = props.title.map(plot::heading);
    let legend = scale.legend(props.unit.as_deref());
    let missing = plot::Legend {
        name: "— 缺失".into(),
        color: MISSING,
    };
    spawn_scene(world, bsn! {
        ChildOf(root)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(8) }
        Children [
            @{title}--
            @WidgetryScrollArea {
                @axis: ScrollAxis::Horizontal,
                @scrollbar_thickness: SCROLLBAR,
                @content: {Some(Box::new(bsn! { on(agentelier_controls::forward_vertical_scroll) }) as Box<dyn Scene>)},
                @children: {Some(Box::new(bsn_list! {
                    Grid Name("HeatmapGrid")
                    Node { display: Display::Grid, width: px(width), height: px(height), min_width: px(width), grid_template_columns: columns, grid_auto_rows: vec![GridTrack::px(CELL_HEIGHT)], column_gap: px(GAP), row_gap: px(GAP), flex_shrink: 0.0 }
                    Children [{cells}]
                }) as Box<dyn SceneList>)}
            }
            Name("HeatmapScroll") Node { width: percent(100), min_width: px(0), height: px(scroll_height), flex_shrink: 0.0 }--
            @legend--
            @plot::legend(missing, 14.0, 14.0)
        ]
    }).map_err(construction)?;
    let grid = find_grid(world, root).ok_or_else(|| construction("热力图缺少矩阵实体"))?;
    let cells = world
        .get::<Children>(grid)
        .into_iter()
        .flatten()
        .filter_map(|id| world.get::<DataCell>(*id).map(|cell| (cell.0, *id)))
        .collect();
    world.entity_mut(root).insert(Heatmap {
        rows: props.rows,
        cells,
    });
    filter_row(world, root, props.selected_row.as_deref()).map_err(construction)
}

fn label_cell(label: &str, background: Color, color: Color) -> Box<dyn Scene> {
    Box::new(cell(label.to_owned(), background, color))
}

fn cell(label: String, background: Color, color: Color) -> impl Scene {
    bsn! {
        Text(label) BackgroundColor(background)
        TextFont { font_size: FontSize::Px(15.0) }
        TextLayout { justify: Justify::Center, linebreak: LineBreak::WordOrCharacter }
        LineHeight::Px(17.0)
        @WidgetryText {
            @colors: {WidgetryTextColorOverrides {
                normal: WidgetryTextStateColorOverrides { foreground: Some(color) },
                ..default()
            }},
        }
        Node { min_width: px(0), height: px(CELL_HEIGHT), padding: UiRect { left: px(4), right: px(4), top: px(5), bottom: px(3) }, overflow: Overflow::clip() }
    }
}

fn find_grid(world: &World, root: Entity) -> Option<Entity> {
    let mut pending = vec![root];
    while let Some(entity) = pending.pop() {
        if world.get::<Grid>(entity).is_some() {
            return Some(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            pending.extend(children.iter());
        }
    }
    None
}

pub(in crate::agent_scenes) fn filter_row(
    world: &mut World,
    root: Entity,
    selected: Option<&str>,
) -> Result {
    let chart = world
        .get::<Heatmap>(root)
        .ok_or_else(|| io::Error::other("热力图缺少行状态"))?;
    let current = selected.and_then(|name| chart.rows.iter().position(|label| label == name));
    let cells = chart.cells.clone();
    for (row, entity) in cells {
        let mut target = world
            .get_mut::<Visibility>(entity)
            .ok_or_else(|| io::Error::other("热力图缺少单元可见性"))?;
        target.set_if_neq(if current.is_none_or(|index| index == row) {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    Ok(())
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    choices: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
) -> Result {
    let props: Properties = serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = props.rows.iter().map(String::as_str).collect();
    if all.is_some_and(|value| names.contains(value))
        || choices
            .into_iter()
            .any(|value| Some(value) != all && !names.contains(value))
    {
        return Err(io::Error::other("热力图筛选选项必须对应行名，全部选项不能与行名重复").into());
    }
    Ok(())
}

fn valid_axis(axis: &[String]) -> bool {
    let mut names = BTreeSet::new();
    (1..=MAX_AXIS).contains(&axis.len())
        && axis
            .iter()
            .all(|name| valid_label(name, 48) && names.insert(name))
}

fn valid_label(value: &str, max: usize) -> bool {
    !value.trim().is_empty()
        && value.len() <= max
        && value.chars().all(super::super::text_input::single_line)
}

fn ramp(fraction: f32) -> Color {
    if fraction <= 0.5 {
        LOW.to_srgba().mix(&MID.to_srgba(), fraction * 2.0).into()
    } else {
        MID.to_srgba()
            .mix(&HIGH.to_srgba(), (fraction - 0.5) * 2.0)
            .into()
    }
}
