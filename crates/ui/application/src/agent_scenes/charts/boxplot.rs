use super::{
    axis::{Axis, AxisKind, AxisSpec, Range},
    construction, invalid,
    plot::{self, Content, Legend, Mark, RetainedPlot, Style, display_number},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::{math::DVec2, prelude::*};
use bevy_widgetry::{canvas::CanvasPathCommand, scene::spawn_scene, text::WidgetryText};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.box_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_label: String,
    y_axis: AxisSpec<f64>,
    groups: Vec<Group>,
    selected_series: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Group {
    name: String,
    color: Option<[u8; 3]>,
    data: Data,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Data {
    Samples {
        samples: Vec<Option<f64>>,
    },
    Summary {
        quartiles: [f64; 3],
        whiskers: [f64; 2],
        quartile_method: String,
        whisker_rule: String,
        outliers: Option<Vec<f64>>,
    },
}

struct Statistics {
    quartiles: [f64; 3],
    whiskers: [f64; 2],
    outliers: Option<Vec<f64>>,
}

struct Distribution {
    legend: Legend,
    statistics: Option<Statistics>,
    source: String,
}

struct Prepared {
    content: Content,
    details: Vec<(String, String)>,
}

#[derive(Component)]
pub(in crate::agent_scenes) struct BoxPlot {
    details: Vec<(String, Entity)>,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Display numeric distributions as native box plots with explicit quartile and whisker definitions.",
        &[
            ("title", "Optional nonblank title."),
            ("x_label", "Required label for the categorical group axis."),
            ("y_axis", "Required {label, unit?, range?}. Optional increasing numeric range. Automatic range includes whiskers and supplied outliers."),
            ("groups", "Required 1 to 8 ordered [{name,color?,data}], color is [R,G,B]. data is {kind:'samples',samples:[number or null,...]} (Tukey hinges and 1.5 IQR whiskers ending at observed inliers), or {kind:'summary',quartiles:[Q1,median,Q3],whiskers:[low,high],quartile_method,whisker_rule,outliers?}. Omit outliers for unknown summary outliers."),
            ("selected_series", "Optional group name; null or omitted shows all. A composition can set this using series_filter."),
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
    let selected = props.selected_series.clone();
    let Prepared { content, details } = prepare(props).map_err(invalid)?;
    let root = RetainedPlot::build(world, parent, content, Style::BoxPlot)?;
    let result = add_details(world, root, details).and_then(|()| {
        super::select_series(world, root, selected.as_deref()).map_err(construction)
    });
    if let Err(error) = result {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

fn add_details(
    world: &mut World,
    root: Entity,
    details: Vec<(String, String)>,
) -> std::result::Result<(), EcsError> {
    let mut rows = Vec::with_capacity(details.len());
    for (index, (name, summary)) in details.into_iter().enumerate() {
        let tag = format!("BoxPlotDetail-{index}");
        let entity = spawn_scene(
            world,
            bsn! {
                ChildOf(root) Name(tag) Text(summary) WidgetryText
                TextFont { font_size: bevy::text::FontSize::Px(16.0) }
                TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
                Node { width: percent(100), min_width: px(0) }
            },
        )
        .map_err(construction)?;
        rows.push((name, entity));
    }
    world.entity_mut(root).insert(BoxPlot { details: rows });
    Ok(())
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let selected = props.selected_series.clone();
    let Prepared { content, details } = prepare(props)?;
    if !RetainedPlot::update(world, root, &content)? {
        return Ok(false);
    }
    let rows = instance(world, root)?.details.clone();
    for ((_, entity), (_, summary)) in rows.into_iter().zip(details) {
        let mut text = world
            .get_mut::<Text>(entity)
            .ok_or_else(|| io::Error::other("箱线图缺少统计明细"))?;
        text.set_if_neq(Text(summary));
    }
    super::select_series(world, root, selected.as_deref())?;
    Ok(true)
}

fn prepare(props: Properties) -> Result<Prepared> {
    let Properties {
        title,
        x_label,
        y_axis,
        groups,
        selected_series,
    } = props;
    let mut unique = BTreeSet::new();
    if !(1..=plot::PALETTE.len()).contains(&groups.len())
        || groups
            .iter()
            .any(|group| !valid_label(&group.name, 96) || !unique.insert(group.name.as_str()))
    {
        return Err(io::Error::other("groups 需要 1 至 8 个唯一的非空单行标签").into());
    }
    if selected_series
        .as_deref()
        .is_some_and(|name| !unique.contains(name))
    {
        return Err(io::Error::other("selected_series 缺少对应分组").into());
    }
    let labels: Vec<_> = groups.iter().map(|group| group.name.clone()).collect();
    let x = Axis::categories(x_label, &labels)?;
    let distributions = groups
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let color = group
                .color
                .map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b));
            let (statistics, source) = group.data.statistics()?;
            Ok(Distribution {
                legend: Legend {
                    name: group.name,
                    color,
                },
                statistics,
                source,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let observations = || {
        distributions
            .iter()
            .filter_map(|group| group.statistics.as_ref())
            .flat_map(Statistics::values)
    };
    let y_range = Range::new(y_axis.range, observations(), AxisKind::Number)?;
    if observations().any(|value| !y_range.fraction(value).is_finite()) {
        return Err(io::Error::other("观测位置无法按 Y 轴建立有限比例").into());
    }
    let y = Axis::new(y_axis.label, y_axis.unit, y_range, AxisKind::Number, 5)?;
    let mut details = Vec::new();
    let marks = distributions
        .into_iter()
        .enumerate()
        .map(|(index, group)| {
            let mut commands = Vec::new();
            let mut detail = format!("{} · {}", group.legend.name, group.source);
            if let Some(stats) = &group.statistics {
                stats.draw(&mut commands, index as f64 + 0.5, x.range, y.range);
                detail.push('\n');
                detail.push_str(&stats.detail());
            }
            details.push((group.legend.name.clone(), detail));
            let mut paint = plot::stroke_paint(group.legend.color, 1.9);
            paint.fill = Some(group.legend.color.with_alpha(0.38).into());
            Mark {
                symbol: plot::LegendSymbol::Column,
                legend: group.legend,
                commands,
                paint,
            }
        })
        .collect();
    Ok(Prepared {
        content: Content {
            title,
            x,
            y,
            marks,
            baseline: None,
        },
        details,
    })
}

impl Data {
    fn statistics(self) -> Result<(Option<Statistics>, String)> {
        match self {
            Self::Samples { samples } => {
                let submitted = samples.len();
                let mut samples: Vec<_> = samples
                    .into_iter()
                    .flatten()
                    .filter(|value| value.is_finite())
                    .collect();
                let valid = samples.len();
                let missing = submitted - valid;
                if valid == 0 {
                    return Ok((None, format!("有效观测 0，缺失 {missing}")));
                }
                samples.sort_by(f64::total_cmp);
                let half = valid.div_ceil(2);
                let low = median(&samples[..half]);
                let high = median(&samples[valid - half..]);
                let quartiles = [low, median(&samples), high];
                let iqr = high - low;
                let lower = samples.partition_point(|value| *value < low - 1.5 * iqr);
                let upper = samples.partition_point(|value| *value <= high + 1.5 * iqr);
                let whiskers = [samples[lower], samples[upper - 1]];
                let outliers = samples[..lower]
                    .iter()
                    .chain(&samples[upper..])
                    .copied()
                    .collect();
                let source =
                    format!("Tukey hinges · 1.5 IQR 内最外侧观测 · 纳入 {valid} · 缺失 {missing}");
                Ok((
                    Some(Statistics {
                        quartiles,
                        whiskers,
                        outliers: Some(outliers),
                    }),
                    source,
                ))
            }
            Self::Summary {
                quartiles,
                whiskers,
                quartile_method,
                whisker_rule,
                outliers,
            } => {
                if !valid_label(&quartile_method, 96) || !valid_label(&whisker_rule, 96) {
                    return Err(io::Error::other("摘要必须注明有效四分位数方法和须线规则").into());
                }
                let arranged = [
                    whiskers[0],
                    quartiles[0],
                    quartiles[1],
                    quartiles[2],
                    whiskers[1],
                ];
                if arranged.iter().any(|v| !v.is_finite())
                    || arranged.windows(2).any(|w| w[0] > w[1])
                {
                    return Err(io::Error::other(
                        "摘要必须满足下须 ≤ Q1 ≤ 中位数 ≤ Q3 ≤ 上须，值必须有限",
                    )
                    .into());
                }
                if outliers.as_ref().is_some_and(|points| {
                    points.iter().any(|point| {
                        !point.is_finite() || (whiskers[0]..=whiskers[1]).contains(point)
                    })
                }) {
                    return Err(io::Error::other("摘要的异常值必须有限并位于须线之外").into());
                }
                let source = format!("统计摘要 · {quartile_method} · {whisker_rule}");
                Ok((
                    Some(Statistics {
                        quartiles,
                        whiskers,
                        outliers,
                    }),
                    source,
                ))
            }
        }
    }
}

impl Statistics {
    fn values(&self) -> impl Iterator<Item = f64> + '_ {
        self.whiskers
            .into_iter()
            .chain(self.outliers.iter().flatten().copied())
    }

    fn detail(&self) -> String {
        let [q1, center, q3] = self.quartiles.map(display_number);
        let [low, high] = self.whiskers.map(display_number);
        let outliers = self.outliers.as_ref().map_or_else(
            || "异常点未提供".into(),
            |points| format!("异常点 {}", points.len()),
        );
        format!("下须 {low} · Q1 {q1} · 中位数 {center} · Q3 {q3} · 上须 {high} · {outliers}")
    }

    fn draw(&self, commands: &mut Vec<CanvasPathCommand>, center: f64, x: Range, y: Range) {
        let left = x.fraction(center - 0.22);
        let right = x.fraction(center + 0.22);
        let center = x.fraction(center);
        let [q1, median, q3] = self.quartiles.map(|value| y.fraction(value));
        let [low, high] = self.whiskers.map(|value| y.fraction(value));
        plot::append_bar(commands, left, right, q1, q3);
        if q1 != q3 {
            plot::append_line(
                commands,
                &[DVec2::new(left, median), DVec2::new(right, median)],
            );
        }
        let cap = (right - left) * 0.27;
        for (whisker, hinge) in [(low, q1), (high, q3)] {
            if whisker != hinge {
                plot::append_line(
                    commands,
                    &[DVec2::new(center, hinge), DVec2::new(center, whisker)],
                );
                plot::append_line(
                    commands,
                    &[
                        DVec2::new(center - cap, whisker),
                        DVec2::new(center + cap, whisker),
                    ],
                );
            }
        }
        if let Some(outliers) = &self.outliers {
            let mut seen = BTreeSet::new();
            for &value in outliers {
                let fraction = y.fraction(value);
                if (0.0..=1.0).contains(&fraction) && seen.insert(fraction.to_bits()) {
                    draw_outlier(commands, plot::map(DVec2::new(center, fraction)));
                }
            }
        }
    }
}

fn draw_outlier(commands: &mut Vec<CanvasPathCommand>, at: Vec2) {
    use CanvasPathCommand::{ArcTo, Close, MoveTo};
    let radii = Vec2::splat(3.0);
    let left = at - Vec2::X * radii.x;
    let right = at + Vec2::X * radii.x;
    commands.extend([
        MoveTo(left),
        ArcTo {
            radii,
            rotation: 0.0,
            large_arc: false,
            sweep: true,
            to: right,
        },
        ArcTo {
            radii,
            rotation: 0.0,
            large_arc: false,
            sweep: true,
            to: left,
        },
        Close,
    ]);
}

fn median(samples: &[f64]) -> f64 {
    let half = samples.len() / 2;
    if samples.len().is_multiple_of(2) {
        samples[half - 1].midpoint(samples[half])
    } else {
        samples[half]
    }
}

fn valid_label(label: &str, limit: usize) -> bool {
    !label.trim().is_empty()
        && label.len() <= limit
        && label.chars().all(super::super::text_input::single_line)
}

fn instance(world: &World, root: Entity) -> Result<&BoxPlot> {
    world
        .get::<BoxPlot>(root)
        .ok_or_else(|| io::Error::other("箱线图缺少分组状态").into())
}

pub(super) fn select_detail(world: &mut World, root: Entity, selected: Option<&str>) -> Result {
    let rows = instance(world, root)?.details.clone();
    for (name, entity) in rows {
        let mut node = world
            .get_mut::<Node>(entity)
            .ok_or_else(|| io::Error::other("箱线图分组缺少明细布局"))?;
        if selected.is_none_or(|selection| selection == name) {
            node.display = Display::Flex;
        } else {
            node.display = Display::None;
        }
    }
    Ok(())
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    choices: impl IntoIterator<Item = &'a str>,
    all: Option<&str>,
) -> Result {
    let props: Properties = serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let names: BTreeSet<_> = props.groups.iter().map(|item| item.name.as_str()).collect();
    if all.is_some_and(|name| names.contains(name))
        || choices
            .into_iter()
            .any(|name| Some(name) != all && !names.contains(name))
    {
        return Err(
            io::Error::other("筛选选项必须对应箱线图分组，且全部选项不能与分组名相同").into(),
        );
    }
    Ok(())
}
