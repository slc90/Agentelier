mod projection;

use super::{
    super::{
        axis::{Axis, AxisKind, AxisSpec, Range},
        construction, invalid,
        plot::{self, RetainedPlot, Style},
    },
    Variables,
    expression::{Expression, tex_number},
    numeric_axis,
    space_view::{self, Axes, SpaceView, ViewAngles, ViewMount},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneState, SceneStored};
use agentelier_markdown::MarkdownView;
use bevy::{math::DVec3, prelude::*};
use bevy_widgetry::scene::spawn_scene;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io};

pub(super) const TEMPLATE: &str = "agentelier.vector_field";
const COLOR: [u8; 3] = [37, 103, 105];
const MAX_VECTORS: usize = 512;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    title: Option<String>,
    x_axis: AxisSpec<f64>,
    y_axis: AxisSpec<f64>,
    z_axis: Option<AxisSpec<f64>>,
    vectors: Option<Vec<Observation>>,
    field: Option<FieldSpec>,
    #[serde(default)]
    scale: Scale,
    vector_unit: Option<String>,
    view: Option<ViewAngles>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    parameters: BTreeMap<String, f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    at: Vec<f64>,
    components: Vec<f64>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FieldSpec {
    components: Vec<String>,
    domain: Option<Vec<[f64; 2]>>,
    density: Option<Vec<usize>>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Scale {
    Component { factor: f64 },
    Unit { length: f64 },
}

impl Default for Scale {
    fn default() -> Self {
        Self::Component { factor: 1.0 }
    }
}

enum Coordinates {
    Plane {
        x: Axis,
        y: Axis,
        domain: [[f64; 2]; 2],
    },
    Space(Box<Axes>),
}

enum Data {
    Observed(Vec<(DVec3, DVec3)>),
    Sampled {
        expressions: Vec<Expression>,
        locations: Vec<DVec3>,
    },
}

struct Program {
    title: Option<String>,
    coordinates: Coordinates,
    data: Data,
    variables: Variables,
    scale: Scale,
    units: Option<String>,
    view: Option<ViewAngles>,
}

#[derive(Component)]
pub(super) struct Field {
    program: Program,
    formula: Entity,
}

#[derive(Clone, Copy)]
enum Mark {
    Arrow { from: DVec3, to: DVec3 },
    Zero(DVec3),
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        TEMPLATE,
        "Draw observed math vectors or a sampled vector field in a plane or a rotatable three-dimensional coordinate frame.",
        &[
            ("title", "Optional nonblank title."),
            ("x_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("y_axis", "Required {label,unit?,range:[min,max]} with finite increasing numeric limits."),
            ("z_axis", "Optional axis with the same schema. Its presence selects 3D, otherwise 2D."),
            ("vectors", "One of vectors and field is required. vectors is 1 to 512 observations {at:[x,y(,z)],components:[u,v(,w)]}, all finite."),
            ("field", "{components:[u,v(,w)],domain?,density?}; evaluate mathematical component expressions in x,y and z for 3D. Optional domain is [[min,max],...] in axis order, default display ranges. density is sample counts per axis, product at most 512, defaults to [9,9] or [5,5,5]. Samples are taken at cell centers."),
            ("scale", "Optional {kind:'component',factor:positive} applies a uniform display multiplier to the original components, default 1. {kind:'unit',length:positive} gives nonzero vectors one length in coordinate units."),
            ("vector_unit", "Optional unit used to describe original vector components, such as m/s."),
            ("parameters", "Optional map of finite numerical variables other than coordinates. Bind with math_parameter."),
            ("view", "Optional spatial initial {yaw,pitch} in degrees, with in-place orbit and saved view state."),
        ],
        build, None, Some(prepare),
    )?.with_conditional_ecs_scene_update(TEMPLATE, patch)?
      .with_ecs_scene_restore(TEMPLATE, restore, prepare_unmounted)
}

fn build(world: &mut World, parent: Entity, props: Properties) -> Result<Entity, EcsError> {
    let program = Program::new(props).map_err(invalid)?;
    let marks = program.marks();
    let root = match &program.coordinates {
        Coordinates::Plane { x, y, .. } => RetainedPlot::build(
            world,
            parent,
            projection::planar(program.title.clone(), x.clone(), y.clone(), &marks),
            Style::VectorField,
        )?,
        Coordinates::Space(axes) => SpaceView::mount(
            world,
            parent,
            ViewMount {
                title: program.title.as_deref(),
                name: "NativeSpatialVectorField",
                axes,
                initial: program.view.unwrap_or_default(),
                geometry: projection::spatial(&marks),
                status: "拖动可旋转视角。".into(),
            },
        )?,
    };
    let source = program.formula(&marks);
    match spawn_scene(world, bsn! {
        ChildOf(root) Name("VectorFieldFormula") @MarkdownView { @source: source, @source_controls: false }
    }).map_err(construction) {
        Ok(formula) => { world.entity_mut(root).insert(Field { program, formula }); Ok(root) }
        Err(error) => { world.despawn(root); Err(error) }
    }
}

fn patch(world: &mut World, root: Entity, props: Properties) -> Result<bool> {
    let program = Program::new(props)?;
    let old = world
        .get::<Field>(root)
        .ok_or_else(|| io::Error::other("向量场缺少定义"))?;
    if old.program.title != program.title || !program.coordinates.matches(&old.program.coordinates)
    {
        return Ok(false);
    }
    let formula = old.formula;
    let marks = program.marks();
    apply(world, root, &program, &marks)?;
    super::set_formula(world, formula, program.formula(&marks))?;
    if let Some(view) = program.view {
        SpaceView::set_angles(world, root, view)?;
    }
    world.entity_mut(root).insert(Field { program, formula });
    Ok(true)
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if props.z_axis.is_some() && props.view.is_none() {
        props.view = world.get::<ViewAngles>(root).copied();
    }
    Ok(props)
}

fn restore(mut props: Properties, saved: &SceneState) -> Result<Properties> {
    if props.z_axis.is_some() {
        props.view = Some(space_view::orientation(&saved.values)?);
    }
    Ok(props)
}

fn prepare_unmounted(mut props: Properties, old: &SceneStored) -> Result<Properties> {
    if props.z_axis.is_some() && props.view.is_none() {
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
    let (marks, formula, source, drawing) = {
        let mut state = world
            .get_mut::<Field>(root)
            .ok_or_else(|| io::Error::other("向量场缺少数学参数"))?;
        if !state.program.variables.set(name, value)? {
            return Ok(());
        }
        let marks = state.program.marks();
        let source = state.program.formula(&marks);
        let drawing = match &state.program.coordinates {
            Coordinates::Plane { x, y, .. } => Some(projection::planar(
                state.program.title.clone(),
                x.clone(),
                y.clone(),
                &marks,
            )),
            Coordinates::Space(_) => None,
        };
        (marks, state.formula, source, drawing)
    };
    if let Some(drawing) = drawing {
        update_planar(world, root, &drawing)?;
    } else {
        SpaceView::update(
            world,
            root,
            projection::spatial(&marks),
            "拖动可旋转视角。".into(),
        )?;
    }
    super::set_formula(world, formula, source)
}

fn apply(world: &mut World, root: Entity, program: &Program, marks: &[Mark]) -> Result {
    match &program.coordinates {
        Coordinates::Plane { x, y, .. } => update_planar(
            world,
            root,
            &projection::planar(program.title.clone(), x.clone(), y.clone(), marks),
        ),
        Coordinates::Space(_) => SpaceView::update(
            world,
            root,
            projection::spatial(marks),
            "拖动可旋转视角。".into(),
        ),
    }
}

fn update_planar(world: &mut World, root: Entity, drawing: &plot::Content) -> Result {
    if !RetainedPlot::update(world, root, drawing)? {
        return Err(io::Error::other("向量场更新期间绘图区结构已变化").into());
    }
    Ok(())
}

impl Coordinates {
    fn new(x: AxisSpec<f64>, y: AxisSpec<f64>, z: Option<AxisSpec<f64>>) -> Result<Self> {
        if let Some(z) = z {
            return Ok(Self::Space(Box::new(Axes::new([x, y, z])?)));
        }
        let domain = super::implicit::Domain::from_axes(&x, &y)?;
        Ok(Self::Plane {
            x: numeric_axis(x, domain.x)?,
            y: numeric_axis(y, domain.y)?,
            domain: [domain.x, domain.y],
        })
    }

    fn names(&self) -> &'static [&'static str] {
        match self {
            Self::Plane { .. } => &["x", "y"],
            Self::Space(_) => &["x", "y", "z"],
        }
    }

    fn limits(&self) -> &[[f64; 2]] {
        match self {
            Self::Plane { domain, .. } => domain,
            Self::Space(axes) => &axes.limits,
        }
    }

    fn fraction(&self, value: DVec3) -> DVec3 {
        match self {
            Self::Plane { x, y, .. } => {
                DVec3::new(x.range.fraction(value.x), y.range.fraction(value.y), 0.5)
            }
            Self::Space(axes) => DVec3::from_array(std::array::from_fn(|axis| {
                axes.ranges[axis].fraction(value[axis])
            })),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Plane { x, y, .. }, Self::Plane { x: bx, y: by, .. }) => (x, y) == (bx, by),
            (Self::Space(a), Self::Space(b)) => a.matches(b),
            _ => false,
        }
    }
}

impl Program {
    fn new(props: Properties) -> Result<Self> {
        let coordinates = Coordinates::new(props.x_axis, props.y_axis, props.z_axis)?;
        let variables = Variables::new(coordinates.names(), props.parameters)?;
        let count = coordinates.names().len();
        let data = match (props.vectors, props.field) {
            (Some(vectors), None) if (1..=MAX_VECTORS).contains(&vectors.len()) => Data::Observed(
                vectors
                    .iter()
                    .map(|row| Ok((vector(&row.at, count)?, vector(&row.components, count)?)))
                    .collect::<Result<_>>()?,
            ),
            (None, Some(field)) if field.components.len() == count => {
                let expressions = field
                    .components
                    .iter()
                    .map(|source| Expression::parse(source, &variables.names))
                    .collect::<Result<_>>()?;
                Data::Sampled {
                    expressions,
                    locations: locations(coordinates.limits(), field.domain, field.density)?,
                }
            }
            _ => {
                return Err(io::Error::other(
                    "向量图必须指定一组有限向量或与坐标轴维数相同的分量表达式",
                )
                .into());
            }
        };
        let magnitude = match props.scale {
            Scale::Component { factor } => factor,
            Scale::Unit { length } => length,
        };
        if !magnitude.is_finite() || magnitude <= 0.0 {
            return Err(io::Error::other("向量比例或归一化长度必须为有限正数").into());
        }
        if props
            .vector_unit
            .as_ref()
            .is_some_and(|unit| unit.trim().is_empty() || unit.len() > 32)
        {
            return Err(io::Error::other("向量单位必须非空且最多 32 字节").into());
        }
        let view = props.view.map(ViewAngles::validate).transpose()?;
        if view.is_some() && count != 3 {
            return Err(io::Error::other("只有空间向量图可以指定视角").into());
        }
        Ok(Self {
            title: props.title,
            coordinates,
            data,
            variables,
            scale: props.scale,
            units: props.vector_unit,
            view,
        })
    }

    fn marks(&self) -> Vec<Mark> {
        match &self.data {
            Data::Observed(rows) => rows
                .iter()
                .filter_map(|&(at, value)| self.mark(at, value))
                .collect(),
            Data::Sampled {
                expressions,
                locations,
            } => {
                let mut slots = self.variables.values.clone();
                locations
                    .iter()
                    .filter_map(|&at| {
                        slots[..expressions.len()]
                            .copy_from_slice(&at.to_array()[..expressions.len()]);
                        let mut components = DVec3::ZERO;
                        for (axis, expression) in expressions.iter().enumerate() {
                            components[axis] = expression.evaluate(&slots)?;
                        }
                        self.mark(at, components)
                    })
                    .collect()
            }
        }
    }

    fn mark(&self, at: DVec3, value: DVec3) -> Option<Mark> {
        let from = self.coordinates.fraction(at);
        if !from
            .to_array()
            .into_iter()
            .all(|value| (0.0..=1.0).contains(&value))
        {
            return None;
        }
        if value == DVec3::ZERO {
            return Some(Mark::Zero(from));
        }
        let offset = match self.scale {
            Scale::Component { factor } => value * factor,
            Scale::Unit { length } => (value / value.abs().max_element()).normalize() * length,
        };
        let end = self.coordinates.fraction(at + offset);
        let to = projection::clip(from, end)?;
        (to != from).then_some(Mark::Arrow { from, to })
    }

    fn formula(&self, marks: &[Mark]) -> String {
        let mut result = Vec::new();
        if let Data::Sampled { expressions, .. } = &self.data {
            let values = self.variables.tex();
            let coordinates = self.coordinates.names().join(",");
            let entries = expressions
                .iter()
                .map(|expr| expr.tex(&values))
                .collect::<Vec<_>>()
                .join(",\\,");
            result.push(format!(
                r"$$\mathbf{{F}}({coordinates})=\left({entries}\right)$$"
            ));
        }
        let unit = self
            .units
            .as_ref()
            .map_or(String::new(), |unit| format!("原始分量单位 {unit}。"));
        let scale = match self.scale {
            Scale::Component { factor } => {
                format!("显示长度按原分量乘以 ${}$。", tex_number(factor, 0))
            }
            Scale::Unit { length } => {
                format!("非零向量统一为 ${}$ 坐标单位。", tex_number(length, 0))
            }
        };
        result.push(format!("{unit}{scale}零向量以点标示。"));
        if marks.is_empty() {
            result.push("当前范围内没有可显示的向量。".into());
        }
        result.join("\n\n")
    }
}

fn vector(input: &[f64], count: usize) -> Result<DVec3> {
    if input.len() != count || input.iter().any(|value| !value.is_finite()) {
        return Err(io::Error::other("观测位置和向量分量必须有限并与坐标维数一致").into());
    }
    Ok(DVec3::new(
        input[0],
        input[1],
        input.get(2).copied().unwrap_or(0.0),
    ))
}

fn locations(
    limits: &[[f64; 2]],
    domain: Option<Vec<[f64; 2]>>,
    density: Option<Vec<usize>>,
) -> Result<Vec<DVec3>> {
    let count = limits.len();
    let bounds = domain.unwrap_or_else(|| limits.to_vec());
    let density = density.unwrap_or_else(|| vec![if count == 2 { 9 } else { 5 }; count]);
    if bounds.len() != count
        || density.len() != count
        || density.iter().any(|&n| !(1..=MAX_VECTORS).contains(&n))
        || density.iter().product::<usize>() > MAX_VECTORS
    {
        return Err(
            io::Error::other("向量场定义域或样本密度与维数不匹配，最多 512 个采样位置").into(),
        );
    }
    let axes = bounds
        .into_iter()
        .zip(limits)
        .zip(density)
        .map(|((bounds, limits), samples)| {
            Range::new(Some(bounds), [], AxisKind::Number)?;
            let a = bounds[0].max(limits[0]);
            let b = bounds[1].min(limits[1]);
            Ok((0..samples)
                .filter(|_| a < b)
                .map(|index| {
                    let fraction = (index as f64 + 0.5) / samples as f64;
                    a * (1.0 - fraction) + b * fraction
                })
                .collect::<Vec<_>>())
        })
        .collect::<Result<Vec<_>>>()?;
    let z = axes.get(2).map_or(&[0.0][..], Vec::as_slice);
    let mut points = Vec::new();
    for &x in &axes[0] {
        for &y in &axes[1] {
            for &z in z {
                points.push(DVec3::new(x, y, z));
            }
        }
    }
    Ok(points)
}
