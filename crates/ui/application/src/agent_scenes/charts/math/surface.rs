use super::{
    super::{
        axis::{AxisKind, AxisSpec, Range},
        construction, invalid,
    },
    Variables,
    expression::Expression,
    implicit::Domain,
    space_view::{self, Axes, SpaceView, Triangles, ViewAngles, ViewMount, world_point},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneState, SceneStored};
use agentelier_markdown::MarkdownView;
use bevy::{math::DVec3, prelude::*};
use bevy_widgetry::scene::spawn_scene;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

pub(super) const TEMPLATE: &str = "agentelier.surface_plot";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    z_axis: AxisSpec<f64>,
    expression: String,
    domain: Option<Domain>,
    samples: Option<[usize; 2]>,
    view: Option<ViewAngles>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

struct Program {
    title: Option<String>,
    axes: Axes,
    domain: Option<[[f64; 2]; 2]>,
    samples: [usize; 2],
    expression: Expression,
    variables: Variables,
    view: Option<ViewAngles>,
}

#[derive(Component)]
pub(super) struct Surface {
    program: Program,
    formula: Entity,
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Render a native rotatable mathematical surface z=f(x,y), with real coordinate axes and holes at undefined values.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("z_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("expression", "Required mathematical expression in x,y and declared parameters. Uses the function_plot numeric grammar without an equals sign."),
            ("domain", "Optional {x:[min,max],y:[min,max]}; evaluated within its intersection with the display ranges."),
            ("samples", "Optional [columns,rows], each 8 to 96; defaults to [41,41]."),
            ("view", "Optional {yaw,pitch} in degrees. Users can drag the native viewport to orbit. Omit on update to retain the current view."),
            ("parameters", "Optional map of finite numeric variables other than x and y, bindable with math_parameter."),
        ],
        build, None, Some(prepare),
    )?.with_conditional_ecs_scene_update(TEMPLATE, patch)?
      .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let program = Program::new(props).map_err(invalid)?;
    let geometry = program.sample();
    let status = status(&geometry);
    let root = SpaceView::mount(
        world,
        parent,
        ViewMount {
            title: program.title.as_deref(),
            name: "NativeSurfacePlot",
            axes: &program.axes,
            initial: program.view.unwrap_or_default(),
            geometry,
            status,
        },
    )?;
    let source = program.formula();
    let formula = spawn_scene(world, bsn! {
        ChildOf(root) Name("SurfaceFormula") @MarkdownView { @source: source, @source_controls: false }
    }).map_err(construction);
    match formula {
        Ok(formula) => {
            world.entity_mut(root).insert(Surface { program, formula });
            Ok(root)
        }
        Err(error) => {
            world.despawn(root);
            Err(error)
        }
    }
}

fn patch(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let program = Program::new(props)?;
    let old = world
        .get::<Surface>(root)
        .ok_or_else(|| io::Error::other("缺少原生曲面状态"))?;
    if old.program.title != program.title || !old.program.axes.matches(&program.axes) {
        return Ok(false);
    }
    let formula = old.formula;
    apply(world, root, formula, &program)?;
    if let Some(view) = program.view {
        SpaceView::set_angles(world, root, view)?;
    }
    world.entity_mut(root).insert(Surface { program, formula });
    Ok(true)
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if props.view.is_none()
        && let Some(&angles) = world.get::<ViewAngles>(root)
    {
        props.view = Some(angles);
    }
    Ok(props)
}

fn restore(mut props: Properties, saved: &SceneState) -> Result<Properties> {
    props.view = Some(space_view::orientation(&saved.values)?);
    Ok(props)
}

fn prepare_unmounted(mut props: Properties, old: &SceneStored) -> Result<Properties> {
    if props.view.is_none() {
        props.view = if let Some(values) = &old.values {
            Some(space_view::orientation(values)?)
        } else {
            serde_json::from_value::<Properties>(serde_json::Value::Object(
                old.definition.props.clone(),
            ))?
            .view
        };
    }
    Ok(props)
}

pub(super) fn set_variable(world: &mut World, root: Entity, name: &str, value: f64) -> Result {
    let (formula, geometry, source) = {
        let mut state = world
            .get_mut::<Surface>(root)
            .ok_or_else(|| io::Error::other("曲面缺少数学参数"))?;
        if !state.program.variables.set(name, value)? {
            return Ok(());
        }
        (
            state.formula,
            state.program.sample(),
            state.program.formula(),
        )
    };
    let status = status(&geometry);
    SpaceView::update(world, root, geometry, status)?;
    super::set_formula(world, formula, source)
}

fn apply(world: &mut World, root: Entity, formula: Entity, program: &Program) -> Result {
    let geometry = program.sample();
    let status = status(&geometry);
    SpaceView::update(world, root, geometry, status)?;
    super::set_formula(world, formula, program.formula())
}

fn status(geometry: &Triangles) -> String {
    if geometry.indices.is_empty() {
        "当前定义域和显示范围内没有可绘制的曲面。".into()
    } else {
        "拖动可旋转视角。".into()
    }
}

impl Program {
    fn new(props: Properties) -> Result<Self> {
        let axes = Axes::new([props.x_axis, props.y_axis, props.z_axis])?;
        let domain = props.domain.unwrap_or(Domain {
            x: axes.limits[0],
            y: axes.limits[1],
        });
        let domain = [domain.x, domain.y];
        for range in domain {
            Range::new(Some(range), [], AxisKind::Number)?;
        }
        let bounds: [[f64; 2]; 2] = std::array::from_fn(|index| {
            [
                domain[index][0].max(axes.limits[index][0]),
                domain[index][1].min(axes.limits[index][1]),
            ]
        });
        let domain = bounds.iter().all(|row| row[0] < row[1]).then_some(bounds);
        let samples = props.samples.unwrap_or([41, 41]);
        if samples.iter().any(|&sample| !(8..=96).contains(&sample)) {
            return Err(io::Error::other("曲面每个坐标轴需要 8 至 96 个采样点").into());
        }
        let variables = Variables::new(&["x", "y"], props.parameters)?;
        let expression = Expression::parse(&props.expression, &variables.names)?;
        let view = props.view.map(ViewAngles::validate).transpose()?;
        Ok(Self {
            title: props.title,
            axes,
            domain,
            samples,
            expression,
            variables,
            view,
        })
    }

    fn formula(&self) -> String {
        format!("$$z={}$$", self.expression.tex(&self.variables.tex()))
    }

    fn sample(&self) -> Triangles {
        let mut mesh = Triangles::default();
        let Some(domain) = self.domain else {
            return mesh;
        };
        let [width, height] = self.samples;
        let mut slots = self.variables.values.clone();
        let mut evaluate = |column: f64, row: f64| {
            slots[0] = interpolate(domain[0], column / (width - 1) as f64);
            slots[1] = interpolate(domain[1], row / (height - 1) as f64);
            let z = self.expression.evaluate(&slots)?;
            let point = DVec3::new(
                self.axes.ranges[0].fraction(slots[0]),
                self.axes.ranges[1].fraction(slots[1]),
                self.axes.ranges[2].fraction(z),
            );
            point.is_finite().then_some(point)
        };
        let mut points = Vec::with_capacity(width * height);
        for row in 0..height {
            for column in 0..width {
                points.push(evaluate(column as f64, row as f64));
            }
        }
        for row in 0..height - 1 {
            for column in 0..width - 1 {
                let start = row * width + column;
                let Some(center) = evaluate(column as f64 + 0.5, row as f64 + 0.5) else {
                    continue;
                };
                let cell = [
                    points[start],
                    points[start + 1],
                    points[start + width + 1],
                    points[start + width],
                ];
                for i in 0..4 {
                    if let (Some(a), Some(b)) = (cell[i], cell[(i + 1) % 4]) {
                        clip_height(&mut mesh, [a, b, center]);
                    }
                }
            }
        }
        mesh
    }
}

fn interpolate(range: [f64; 2], fraction: f64) -> f64 {
    range[0] * (1.0 - fraction) + range[1] * fraction
}

fn clip_height(mesh: &mut Triangles, triangle: [DVec3; 3]) {
    if triangle.iter().all(|point| (0.0..=1.0).contains(&point.z)) {
        append_face(mesh, triangle);
        return;
    }
    let mut polygon = triangle.to_vec();
    for (edge, retain_above) in [(0.0, true), (1.0, false)] {
        let input = std::mem::take(&mut polygon);
        let Some(&last) = input.last() else {
            return;
        };
        let inside = |point: DVec3| {
            if retain_above {
                point.z >= edge
            } else {
                point.z <= edge
            }
        };
        let mut prior = last;
        for next in input {
            if inside(prior) != inside(next) {
                let slope = prior.z.abs().max(next.z.abs()).max(1.0);
                let mix = (edge / slope - prior.z / slope) / (next.z / slope - prior.z / slope);
                let crossing = prior.truncate().lerp(next.truncate(), mix);
                polygon.push(crossing.extend(edge));
            }
            if inside(next) {
                polygon.push(next);
            }
            prior = next;
        }
    }
    if let Some(&first) = polygon.first() {
        for pair in polygon[1..].windows(2) {
            append_face(mesh, [first, pair[0], pair[1]]);
        }
    }
}

fn append_face(mesh: &mut Triangles, corners: [DVec3; 3]) {
    let points = corners.map(world_point);
    let normal = (points[1] - points[0])
        .cross(points[2] - points[0])
        .normalize_or_zero();
    if normal == Vec3::ZERO {
        return;
    }
    let light = normal.dot(Vec3::new(-0.36, 0.82, 0.43)).max(0.0) * 0.33 + 0.67;
    let colors = corners.map(|point| {
        let z = point.z as f32;
        let cool = Vec3::new(0.035, 0.22, 0.39);
        let warm = Vec3::new(0.36, 0.65, 0.34);
        (cool.lerp(warm, z) * light).extend(1.0).to_array()
    });
    mesh.push(points, colors);
}
