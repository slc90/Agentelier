use super::{
    axis::{Axis, AxisKind, AxisSpec, Range},
    construction, invalid,
    plot::{self, Content, Legend, Mark, RetainedPlot, Style, display_number},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use bevy::prelude::*;
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, io};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.histogram";
const MAX_BINS: usize = 32;
const MAX_LAYOUTS: usize = 8;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_range: Option<[f64; 2]>,
    #[serde(default)]
    statistic: Statistic,
    data: Data,
    selected_binning: Option<String>,
}

#[derive(Clone, Copy, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum Statistic {
    #[default]
    Count,
    Frequency,
    Density,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Data {
    Samples {
        samples: Vec<Option<f64>>,
        binnings: Vec<Binning>,
    },
    Summary {
        edges: Vec<f64>,
        counts: Vec<u32>,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Binning {
    name: String,
    edges: Vec<f64>,
}

struct Layout {
    name: Option<String>,
    bins: Vec<Bin>,
    total: u64,
    excluded: usize,
    missing: usize,
}

struct Bin {
    lower: f64,
    upper: f64,
    count: u64,
    height: Option<f64>,
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    layouts: Vec<Layout>,
    statistic: Statistic,
}

#[derive(Component)]
struct Histogram {
    data: Prepared,
    detail: Entity,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Show a histogram with real interval widths and a count, relative frequency, or probability density axis.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label, unit?, range?}. Optional visible range; the default axis includes all configured edges."),
            ("y_range", "Optional [minimum, maximum] visible range for the selected statistic."),
            ("statistic", "count (default), frequency (fraction of assigned samples), or density (frequency divided by bin width)."),
            ("data", "Required {kind:'samples', samples:[number or null,...], binnings:[{name,edges},...]} or {kind:'summary',edges,counts}. Provide 2 to 33 strictly increasing finite edges per binning and one count per summary interval. Bins include the left edge and exclude the right edge, except the last bin includes both. Values outside the edges and null samples are reported separately."),
            ("selected_binning", "Optional samples binning name; defaults to the first one. A composition can bind a choice using histogram_binning. Summary counts have fixed edges."),
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
    let selected = props.selected_binning.clone();
    let histogram = Prepared::new(props).map_err(invalid)?;
    let layout = histogram.layout(selected.as_deref()).map_err(invalid)?;
    let root = RetainedPlot::build(world, parent, histogram.content(layout), Style::Histogram)?;
    let text = layout.detail(histogram.statistic);
    let detail = match spawn_scene(
        world,
        bsn! {
            ChildOf(root) Name("HistogramBins") Text(text) WidgetryText
            TextFont { font_size: bevy::text::FontSize::Px(16.0) }
            TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
            Node { width: percent(100), min_width: px(0) }
        },
    ) {
        Ok(detail) => detail,
        Err(error) => {
            world.despawn(root);
            return Err(construction(error));
        }
    };
    world.entity_mut(root).insert(Histogram {
        data: histogram,
        detail,
    });
    Ok(root)
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let detail = instance(world, root)?.detail;
    let selected = props.selected_binning.clone();
    let data = Prepared::new(props)?;
    let layout = data.layout(selected.as_deref())?;
    if !RetainedPlot::update(world, root, &data.content(layout))? {
        return Ok(false);
    }
    set_detail(world, detail, layout.detail(data.statistic))?;
    world.entity_mut(root).insert(Histogram { data, detail });
    Ok(true)
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let Properties {
            title,
            x_axis,
            y_range,
            statistic,
            data,
            ..
        } = props;
        let layouts = match data {
            Data::Samples { samples, binnings } => sample_layouts(&samples, binnings, statistic)?,
            Data::Summary { edges, counts } => {
                validate_edges(&edges)?;
                if counts.len() != edges.len() - 1 {
                    return Err(io::Error::other("counts 必须与分箱区间逐一对应").into());
                }
                vec![Layout::new(
                    None,
                    &edges,
                    counts.into_iter().map(u64::from).collect(),
                    0,
                    0,
                    statistic,
                )?]
            }
        };
        let bins = || layouts.iter().flat_map(|layout| &layout.bins);
        let x_range = Range::new(
            x_axis.range,
            bins().flat_map(|bin| [bin.lower, bin.upper]),
            AxisKind::Number,
        )?;
        let max_height = bins().filter_map(|bin| bin.height).fold(0.0, f64::max);
        let default_y = [0.0, if max_height > 0.0 { max_height } else { 1.0 }];
        let y_range = Range::new(Some(y_range.unwrap_or(default_y)), [], AxisKind::Number)?;
        if !y_range.fraction(0.0).is_finite()
            || bins().any(|bin| {
                !x_range.fraction(bin.lower).is_finite()
                    || !x_range.fraction(bin.upper).is_finite()
                    || bin
                        .height
                        .is_some_and(|value| !y_range.fraction(value).is_finite())
            })
        {
            return Err(io::Error::other("分箱和数值范围需要有限的坐标比例").into());
        }
        let y_label = statistic.axis_label(x_axis.unit.as_deref());
        let x = Axis::new(x_axis.label, x_axis.unit, x_range, AxisKind::Number, 6)?;
        let y = Axis::new(y_label, None, y_range, AxisKind::Number, 5)?;
        Ok(Self {
            title,
            x,
            y,
            layouts,
            statistic,
        })
    }

    fn layout(&self, selected: Option<&str>) -> Result<&Layout> {
        let result = match selected {
            Some(selected) => self
                .layouts
                .iter()
                .find(|layout| layout.name.as_deref() == Some(selected)),
            None => self.layouts.first(),
        };
        result.ok_or_else(|| io::Error::other("selected_binning 必须对应样本的分箱名称").into())
    }

    fn content(&self, layout: &Layout) -> Content {
        let mut commands = Vec::new();
        let baseline = self.y.range.fraction(0.0);
        for bin in &layout.bins {
            if let Some(height) = bin.height {
                plot::append_bar(
                    &mut commands,
                    self.x.range.fraction(bin.lower),
                    self.x.range.fraction(bin.upper),
                    baseline,
                    self.y.range.fraction(height),
                );
            }
        }
        let color = plot::palette(0);
        let mut paint = plot::stroke_paint(color, 1.4);
        paint.fill = Some(color.with_alpha(0.66).into());
        let mark = Mark {
            legend: Legend {
                name: "分布".into(),
                color,
            },
            commands,
            paint,
        };
        Content {
            title: self.title.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            marks: vec![mark],
            baseline: None,
        }
    }
}

impl Statistic {
    fn height(self, count: u64, total: u64, width: f64) -> Option<f64> {
        let count = count as f64;
        if matches!(self, Self::Count) {
            return Some(count);
        }
        if total == 0 {
            return None;
        }
        let fraction = count / total as f64;
        Some(if matches!(self, Self::Density) {
            fraction / width
        } else {
            fraction
        })
    }

    fn axis_label(self, unit: Option<&str>) -> String {
        match (self, unit) {
            (Self::Count, _) => "计数".into(),
            (Self::Frequency, _) => "相对频率".into(),
            (Self::Density, Some(unit)) => format!("概率密度 (1/{unit})"),
            (Self::Density, None) => "概率密度".into(),
        }
    }
}

impl Layout {
    fn new(
        name: Option<String>,
        edges: &[f64],
        counts: Vec<u64>,
        excluded: usize,
        missing: usize,
        statistic: Statistic,
    ) -> Result<Self> {
        let total = counts.iter().sum();
        let bins: Vec<_> = edges
            .windows(2)
            .zip(counts)
            .map(|(edge, count)| Bin {
                lower: edge[0],
                upper: edge[1],
                count,
                height: statistic.height(count, total, edge[1] - edge[0]),
            })
            .collect();
        if bins
            .iter()
            .any(|bin| bin.height.is_some_and(|height| !height.is_finite()))
        {
            return Err(io::Error::other("分箱宽度和频数产生不可表示的密度").into());
        }
        Ok(Self {
            name,
            bins,
            total,
            excluded,
            missing,
        })
    }

    fn detail(&self, statistic: Statistic) -> String {
        let prefix = self
            .name
            .as_ref()
            .map_or_else(String::new, |name| format!("{name} · "));
        let mut detail = format!("{prefix}纳入 {}", self.total);
        if self.excluded > 0 {
            detail.push_str(&format!(" · 范围外 {}", self.excluded));
        }
        if self.missing > 0 {
            detail.push_str(&format!(" · 缺失 {}", self.missing));
        }
        if self.total == 0 && !matches!(statistic, Statistic::Count) {
            detail.push_str(" · 无法计算比例");
        }
        for (index, bin) in self.bins.iter().enumerate() {
            let close = if index + 1 == self.bins.len() {
                ']'
            } else {
                ')'
            };
            detail.push_str(&format!(
                "\n[{}, {}{close}   {} 个",
                display_number(bin.lower),
                display_number(bin.upper),
                bin.count
            ));
            if !matches!(statistic, Statistic::Count) {
                let value = bin.height.map_or_else(|| "—".into(), display_number);
                detail.push_str(" · ");
                detail.push_str(&value);
            }
        }
        detail
    }
}

fn sample_layouts(
    samples: &[Option<f64>],
    binnings: Vec<Binning>,
    statistic: Statistic,
) -> Result<Vec<Layout>> {
    let mut names = BTreeSet::new();
    if !(1..=MAX_LAYOUTS).contains(&binnings.len())
        || binnings.iter().any(|layout| {
            layout.name.trim().is_empty()
                || layout.name.len() > 64
                || !names.insert(layout.name.as_str())
                || !layout
                    .name
                    .chars()
                    .all(super::super::text_input::single_line)
        })
    {
        return Err(io::Error::other("binnings 必须包含 1 至 8 个唯一的非空单行名称").into());
    }
    let valid: Vec<_> = samples
        .iter()
        .filter_map(|value| value.filter(|value| value.is_finite()))
        .collect();
    let missing = samples.len() - valid.len();
    binnings
        .into_iter()
        .map(|layout| {
            validate_edges(&layout.edges)?;
            let last = layout.edges.len() - 1;
            let mut counts = vec![0; last];
            let mut outside = 0;
            for sample in &valid {
                if *sample < layout.edges[0] || *sample > layout.edges[last] {
                    outside += 1;
                    continue;
                }
                let right = layout.edges.partition_point(|edge| edge <= sample);
                counts[right.min(last) - 1] += 1;
            }
            Layout::new(
                Some(layout.name),
                &layout.edges,
                counts,
                outside,
                missing,
                statistic,
            )
        })
        .collect()
}

fn validate_edges(edges: &[f64]) -> Result {
    if !(2..=MAX_BINS + 1).contains(&edges.len())
        || edges.windows(2).any(|edge| {
            !edge[0].is_finite()
                || !edge[1].is_finite()
                || edge[0] >= edge[1]
                || !(edge[1] - edge[0]).is_finite()
        })
    {
        return Err(io::Error::other("edges 需要 2 至 33 个递增有限边界及有限区间宽度").into());
    }
    Ok(())
}

pub(in crate::agent_scenes) fn select(world: &mut World, root: Entity, name: &str) -> Result {
    let histogram = instance(world, root)?;
    let layout = histogram.data.layout(Some(name))?;
    let content = histogram.data.content(layout);
    let detail = histogram.detail;
    let text = layout.detail(histogram.data.statistic);
    if !RetainedPlot::update(world, root, &content)? {
        return Err(io::Error::other("切换分箱时直方图布局已变化").into());
    }
    set_detail(world, detail, text)
}

fn instance(world: &World, root: Entity) -> Result<&Histogram> {
    world
        .get::<Histogram>(root)
        .ok_or_else(|| io::Error::other("直方图缺少分箱数据").into())
}

fn set_detail(world: &mut World, detail: Entity, text: String) -> Result {
    let mut target = world
        .get_mut::<Text>(detail)
        .ok_or_else(|| io::Error::other("直方图缺少区间明细"))?;
    target.set_if_neq(Text(text));
    Ok(())
}

pub(in crate::agent_scenes) fn validate_choices<'a>(
    scene: &SceneDefinition,
    values: impl IntoIterator<Item = &'a str>,
) -> Result {
    let props: Properties = serde_json::from_value(serde_json::Value::Object(scene.props.clone()))?;
    let Data::Samples { binnings, .. } = props.data else {
        return Err(io::Error::other("只有样本能够切换分箱边界").into());
    };
    if values
        .into_iter()
        .any(|value| !binnings.iter().any(|binning| binning.name == value))
    {
        return Err(io::Error::other("分箱选择必须对应分箱名称").into());
    }
    Ok(())
}
