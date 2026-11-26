use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        construction, invalid,
        plot::{self, Content, Legend, LegendSymbol, Mark, RetainedPlot, Style},
    },
    expression::{Expression, valid_parameter},
    trace::trace,
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use agentelier_markdown::{MarkdownSource, MarkdownView};
use bevy::{math::DVec2, prelude::*, ui::UiSystems};
use bevy_widgetry::scene::spawn_scene;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    io,
};

pub(in crate::agent_scenes) const TEMPLATE: &str = "agentelier.function_plot";
const DEFAULT_STEPS: usize = 352;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    curves: Vec<Curve>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    points: Vec<PointSeries>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Curve {
    name: String,
    expression: String,
    color: Option<[u8; 3]>,
    domain: Option<[f64; 2]>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PointSeries {
    name: String,
    color: Option<[u8; 3]>,
    points: Vec<[f64; 2]>,
}

struct CurveProgram {
    legend: Legend,
    expression: Expression,
    domain: [f64; 2],
}

struct Prepared {
    title: Option<String>,
    x: Axis,
    y: Axis,
    x_domain: [f64; 2],
    curves: Vec<CurveProgram>,
    points: Vec<(Legend, Vec<[f64; 2]>)>,
    names: Vec<String>,
    values: Vec<f64>,
}

#[derive(Component)]
struct FunctionPlot {
    plot: Prepared,
    formula: Entity,
    steps: usize,
}

#[derive(Component)]
struct FunctionViewport(Entity);

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Plot explicit mathematical functions with chart axes and native typeset formulas. Bind numeric composition parameters locally.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("curves", "One or more [{name,expression,domain?,color?}]. Expression uses x, numeric parameters, pi/tau/e, operators + - * / ^, parentheses and functions sin, cos, tan, sqrt, abs, exp, ln, log10, min, max and related elementary functions. Use explicit multiplication and no equals sign. Domain is a finite increasing [min,max]; it defaults to the X display range. color is [R,G,B]."),
            ("points", "Optional independent point series [{name,points:[[x,y],...],color?}]. There may be up to 8 total curves and point series."),
            ("parameters", "Optional map from names such as a/b/c to finite numeric values. Declare each variable used by the curves. A composition can bind a number with kind math_parameter, parameter, and variable."),
        ],
        build,
        None,
        None,
    )?
    .with_conditional_ecs_scene_update(TEMPLATE, update)
}

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, resample_viewports.after(UiSystems::Layout));
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    let plot = Prepared::new(props).map_err(invalid)?;
    let (content, source) = plot.project(DEFAULT_STEPS);
    let root = RetainedPlot::build(world, parent, content, Style::Function)?;
    let result = decorate(world, root, plot, source);
    if let Err(error) = result {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

fn decorate(
    world: &mut World,
    root: Entity,
    plot: Prepared,
    source: String,
) -> std::result::Result<(), EcsError> {
    let viewport = plot::viewport_in(world, root).ok_or_else(|| construction("函数图缺少视口"))?;
    let formula = spawn_scene(world, bsn! {
        ChildOf(root) @MarkdownView { @source: source, @source_controls: false } Name("FunctionFormula")
    }).map_err(construction)?;
    world.entity_mut(root).insert(FunctionPlot {
        plot,
        formula,
        steps: DEFAULT_STEPS,
    });
    world.entity_mut(viewport).insert(FunctionViewport(root));
    Ok(())
}

fn update(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let plot = Prepared::new(props)?;
    let old = instance(world, root)?;
    let (steps, formula) = (old.steps, old.formula);
    let (content, source) = plot.project(steps);
    if !RetainedPlot::update(world, root, &content)? {
        return Ok(false);
    }
    set_formula(world, formula, source)?;
    world.entity_mut(root).insert(FunctionPlot {
        plot,
        formula,
        steps,
    });
    Ok(true)
}

impl Prepared {
    fn new(props: Properties) -> Result<Self> {
        let Properties {
            title,
            x_axis,
            y_axis,
            curves,
            points,
            parameters,
        } = props;
        if curves.is_empty() || curves.len() + points.len() > plot::PALETTE.len() {
            return Err(
                io::Error::other("函数图需要至少一条表达式，曲线和散点总数不超过 8").into(),
            );
        }
        if parameters.len() > 16
            || parameters
                .iter()
                .any(|(name, &value)| name == "x" || !valid_parameter(name) || !value.is_finite())
        {
            return Err(
                io::Error::other("函数参数需使用有效的非 x 变量名及有限数值，最多 16 个").into(),
            );
        }
        let mut unique = BTreeSet::new();
        if curves
            .iter()
            .map(|curve| &curve.name)
            .chain(points.iter().map(|points| &points.name))
            .any(|name| name.trim().is_empty() || name.len() > 96 || !unique.insert(name.as_str()))
        {
            return Err(io::Error::other("曲线和散点系列需要唯一的非空名称，最多 96 字节").into());
        }
        let x_domain = x_axis
            .range
            .ok_or_else(|| io::Error::other("函数图需要显式 X 轴范围"))?;
        let y_domain = y_axis
            .range
            .ok_or_else(|| io::Error::other("函数图需要显式 Y 轴范围"))?;
        let x = numeric_axis(x_axis, x_domain)?;
        let y = numeric_axis(y_axis, y_domain)?;
        let names: Vec<_> = std::iter::once("x".to_owned())
            .chain(parameters.keys().cloned())
            .collect();
        let values = std::iter::once(0.0)
            .chain(parameters.values().copied())
            .collect();
        let curves = curves
            .into_iter()
            .enumerate()
            .map(|(index, curve)| {
                let domain = curve.domain.unwrap_or(x_domain);
                Range::new(Some(domain), [], AxisKind::Number)?;
                let expression = Expression::parse(&curve.expression, &names)
                    .map_err(|error| io::Error::other(format!("曲线 {}：{error}", curve.name)))?;
                Ok(CurveProgram {
                    legend: named_color(curve.name, curve.color, index),
                    expression,
                    domain,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let start = curves.len();
        let points = points
            .into_iter()
            .enumerate()
            .map(|(index, point)| {
                if point
                    .points
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
                {
                    return Err(io::Error::other("散点坐标必须有限").into());
                }
                Ok((
                    named_color(point.name, point.color, start + index),
                    point.points,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            title,
            x,
            y,
            x_domain,
            curves,
            points,
            names,
            values,
        })
    }

    fn project(&self, steps: usize) -> (Content, String) {
        let mut values = self.values.clone();
        let x = self.x.range;
        let y = self.y.range;
        let mut marks: Vec<_> = self
            .curves
            .iter()
            .map(|curve| {
                let [left, right] = [
                    curve.domain[0].max(self.x_domain[0]),
                    curve.domain[1].min(self.x_domain[1]),
                ];
                let mut commands = Vec::new();
                if left <= right {
                    trace(&mut commands, [left, right], steps, |sample| {
                        values[0] = sample;
                        let observed = curve.expression.evaluate(&values)?;
                        let point = DVec2::new(x.fraction(sample), y.fraction(observed));
                        point.is_finite().then_some(point)
                    });
                }
                Mark {
                    legend: curve.legend.clone(),
                    symbol: LegendSymbol::Line,
                    commands,
                    paint: plot::stroke_paint(curve.legend.color, plot::LINE_WIDTH),
                }
            })
            .collect();
        let has_curve = marks.iter().any(|mark| !mark.commands.is_empty());
        for (legend, points) in &self.points {
            let mut commands = Vec::new();
            for &[point_x, point_y] in points {
                plot::append_marker(
                    &mut commands,
                    DVec2::new(x.fraction(point_x), y.fraction(point_y)),
                );
            }
            marks.push(Mark {
                legend: legend.clone(),
                symbol: LegendSymbol::Point,
                commands,
                paint: plot::stroke_paint(legend.color, 7.0),
            });
        }
        let mut source = self.formula();
        if !has_curve {
            source.push_str("\n\n当前范围内没有可绘制的曲线。");
        }
        let content = Content {
            title: self.title.clone(),
            x: self.x.clone(),
            y: self.y.clone(),
            marks,
            baseline: None,
        };
        (content, source)
    }

    fn formula(&self) -> String {
        let slots: Vec<_> = std::iter::once(None)
            .chain(self.values[1..].iter().copied().map(Some))
            .collect();
        self.curves
            .iter()
            .enumerate()
            .map(|(index, curve)| {
                let expression = curve.expression.tex(&slots);
                let name = if self.curves.len() == 1 {
                    "f".into()
                } else {
                    format!("f_{{{}}}", index + 1)
                };
                format!("$${name}(x)={expression}$$")
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

fn numeric_axis(axis: AxisSpec<f64>, domain: [f64; 2]) -> Result<Axis> {
    let range = Range::new(Some(domain), [], AxisKind::Number)?;
    Axis::new(axis.label, axis.unit, range, AxisKind::Number, 5)
}

fn named_color(name: String, color: Option<[u8; 3]>, index: usize) -> Legend {
    Legend {
        name,
        color: color.map_or_else(|| plot::palette(index), |[r, g, b]| Color::srgb_u8(r, g, b)),
    }
}

fn instance(world: &World, root: Entity) -> Result<&FunctionPlot> {
    world
        .get::<FunctionPlot>(root)
        .ok_or_else(|| io::Error::other("函数图缺少表达式状态").into())
}

fn set_formula(world: &mut World, formula: Entity, source: String) -> Result {
    let mut target = world
        .get_mut::<MarkdownSource>(formula)
        .ok_or_else(|| io::Error::other("函数图缺少公式区域"))?;
    target.set_if_neq(MarkdownSource(source));
    Ok(())
}

fn refresh(world: &mut World, root: Entity) -> Result {
    let state = instance(world, root)?;
    let (content, source) = state.plot.project(state.steps);
    let formula = state.formula;
    if !RetainedPlot::update(world, root, &content)? {
        return Err(io::Error::other("函数更新期间绘图区结构已变化").into());
    }
    set_formula(world, formula, source)
}

fn resample_viewports(
    views: Query<(&FunctionViewport, &ComputedNode), Changed<ComputedNode>>,
    mut commands: Commands,
) {
    for (view, layout) in &views {
        if layout.size().x <= 0.0 {
            continue;
        }
        let root = view.0;
        let steps = ((layout.size().x as usize / 3 / 32) * 32).clamp(128, 640);
        commands.queue(move |world: &mut World| -> Result {
            let Some(mut state) = world.get_mut::<FunctionPlot>(root) else {
                return Ok(());
            };
            if state.steps == steps {
                return Ok(());
            }
            state.steps = steps;
            refresh(world, root)
        });
    }
}

pub(super) fn bind_variable(scene: &mut SceneDefinition, variable: &str, value: f64) -> Result {
    let values = scene
        .props
        .get_mut("parameters")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| io::Error::other("函数图必须先声明 parameters"))?;
    let target = values
        .get_mut(variable)
        .ok_or_else(|| io::Error::other(format!("函数图没有参数 {variable}")))?;
    *target = json!(value);
    Ok(())
}

pub(super) fn set_variable(world: &mut World, root: Entity, variable: &str, value: f64) -> Result {
    let mut state = world
        .get_mut::<FunctionPlot>(root)
        .ok_or_else(|| io::Error::other("函数图缺少当前变量"))?;
    let index = state
        .plot
        .names
        .iter()
        .position(|name| name == variable && name != "x")
        .ok_or_else(|| io::Error::other(format!("函数图没有参数 {variable}")))?;
    if state.plot.values[index] == value {
        return Ok(());
    }
    state.plot.values[index] = value;
    refresh(world, root)
}
