use super::{
    axis::{Axis, AxisKind, AxisSpec, Range},
    construction, invalid,
    plot::{self, Baseline, Content, Legend, Mark, RetainedPlot, Style},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.bar_chart";
const MAX_CATEGORIES: usize = 16;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_label: String,
    y_axis: AxisSpec<f64>,
    categories: Vec<String>,
    series: Vec<Series>,
    baseline: f64,
    selected_category: Option<String>,
    selected_series: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Series {
    name: String,
    color: Option<[u8; 3]>,
    values: Vec<Option<f64>>,
}

#[derive(Component)]
struct Bars {
    title: Option<String>,
    x: Axis,
    y: Axis,
    baseline: Baseline,
    categories: Vec<String>,
    series: Vec<Series>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Compare signed amounts in native grouped bars with a categorical axis and an explicit baseline.",
        &[
            ("title", "Optional nonblank title."),
            ("x_label", "Required category axis label."),
            ("y_axis", "Required {label, unit?, range?}. range has two increasing numbers. Automatic range includes valid values and baseline."),
            ("categories", "Required 1 to 16 distinct category names, in display order."),
            ("series", "Required 1 to 8 named series [{name, color?, values}]. Values follow categories, one numeric value or null per category. Null leaves an empty slot; a value at the baseline has a colored mark. color is [R,G,B]."),
            ("baseline", "Required finite numeric baseline in the units of the Y axis."),
            ("selected_category", "Optional category name. Null or omitted includes all. A composition can use category_filter."),
            ("selected_series", "Optional series name. Null or omitted includes all. A composition can use series_filter."),
        ],
        build,
        None,
        None,
    )?
    .with_conditional_ecs_scene_update(TEMPLATE, update)
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    let category = props.selected_category.clone();
    let series = props.selected_series.clone();
    let bars = Bars::prepare(props).map_err(invalid)?;
    let content = bars.content(category.as_deref());
    let root = RetainedPlot::build(world, parent, content, Style::Bars)?;
    world.entity_mut(root).insert(bars);
    if let Err(error) = RetainedPlot::select_series(world, root, series.as_deref()) {
        world.despawn(root);
        return Err(construction(error));
    }
    Ok(root)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let category = props.selected_category.clone();
    let series = props.selected_series.clone();
    let bars = Bars::prepare(props)?;
    if !RetainedPlot::update(world, root, &bars.content(category.as_deref()))? {
        return Ok(false);
    }
    RetainedPlot::select_series(world, root, series.as_deref())?;
    world.entity_mut(root).insert(bars);
    Ok(true)
}

impl Bars {
    fn prepare(props: Properties) -> Result<Self> {
        let Properties {
            title,
            x_label,
            y_axis,
            categories,
            series,
            baseline,
            selected_category,
            selected_series,
        } = props;
        if !(1..=MAX_CATEGORIES).contains(&categories.len())
            || !distinct(categories.iter().map(String::as_str))
        {
            return Err(
                io::Error::other("categories 必须包含 1 至 16 个唯一的非空单行名称").into(),
            );
        }
        if !(1..=plot::PALETTE.len()).contains(&series.len())
            || !distinct(series.iter().map(|item| item.name.as_str()))
            || series
                .iter()
                .any(|item| item.values.len() != categories.len())
        {
            return Err(io::Error::other(
                "series 需要 1 至 8 个唯一非空名称，且每组值与类别逐一对应",
            )
            .into());
        }
        if selected_category
            .as_ref()
            .is_some_and(|name| !categories.contains(name))
            || selected_series
                .as_ref()
                .is_some_and(|name| !series.iter().any(|item| &item.name == name))
        {
            return Err(io::Error::other("选择的类别或系列不存在").into());
        }
        let x = Axis::categories(x_label, &categories)?;
        let values = || {
            series
                .iter()
                .flat_map(|item| item.values.iter().copied().flatten())
        };
        let range = Range::new(y_axis.range, values().chain([baseline]), AxisKind::Number)?;
        let fraction = range.fraction(baseline);
        if !fraction.is_finite() || values().any(|value| !range.fraction(value).is_finite()) {
            return Err(io::Error::other("量值和基线必须能映射到有限的轴比例").into());
        }
        let y = Axis::new(y_axis.label, y_axis.unit, range, AxisKind::Number, 5)?;
        Ok(Self {
            title,
            x,
            y,
            baseline: Baseline {
                fraction,
                value: baseline,
            },
            categories,
            series,
        })
    }

    fn content(&self, selected: Option<&str>) -> Content {
        let count = self.series.len() as f64;
        let slot_width = 0.72 / count;
        let marks = self
            .series
            .iter()
            .enumerate()
            .map(|(index, series)| {
                let mut commands = Vec::new();
                for (group, (category, value)) in
                    self.categories.iter().zip(&series.values).enumerate()
                {
                    let Some(value) =
                        value.filter(|_| selected.is_none_or(|name| name == category))
                    else {
                        continue;
                    };
                    let center = group as f64 + 0.14 + slot_width * (index as f64 + 0.5);
                    let left = self.x.range.fraction(center - slot_width * 0.42);
                    let right = self.x.range.fraction(center + slot_width * 0.42);
                    let base = self.baseline.fraction;
                    plot::append_bar(
                        &mut commands,
                        left,
                        right,
                        base,
                        self.y.range.fraction(value),
                    );
                }
                let color = series
                    .color
                    .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b));
                let mut paint = plot::stroke_paint(color, 1.4);
                paint.fill = Some(color.with_alpha(0.76).into());
                Mark {
                    legend: Legend {
                        name: series.name.clone(),
                        color,
                    },
                    commands,
                    paint,
                }
            })
            .collect();
        Content {
            title: self.title.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            marks,
            baseline: Some(self.baseline),
        }
    }
}

fn distinct<'a>(names: impl IntoIterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    names.into_iter().all(|name| {
        !name.trim().is_empty()
            && name.len() <= 96
            && name.chars().all(super::super::text_input::single_line)
            && seen.insert(name)
    })
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    choices: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
    by_category: bool,
) -> Result {
    let properties: Properties =
        serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = if by_category {
        properties.categories.iter().map(String::as_str).collect()
    } else {
        properties
            .series
            .iter()
            .map(|series| series.name.as_str())
            .collect()
    };
    if all.is_some_and(|all| names.contains(all))
        || choices
            .into_iter()
            .any(|value| Some(value) != all && !names.contains(value))
    {
        return Err(
            io::Error::other("筛选选项必须对应类别或系列，且全部选项不能与数据名称重复").into(),
        );
    }
    Ok(())
}

pub(in crate::agent_scenes) fn filter_category(
    world: &mut World,
    root: Entity,
    selected: Option<&str>,
) -> Result {
    let bars = world
        .get::<Bars>(root)
        .ok_or_else(|| io::Error::other("柱状图缺少分组数据"))?;
    let content = bars.content(selected);
    if !RetainedPlot::update(world, root, &content)? {
        return Err(io::Error::other("筛选时柱状图布局已变化").into());
    }
    Ok(())
}
