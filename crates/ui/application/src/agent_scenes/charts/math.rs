mod expression;
mod function;
mod implicit;
mod level_grid;
mod parametric;
mod polar;
mod region;
mod region_mesh;
mod trace;

use super::{
    axis::{Axis, AxisKind, AxisSpec, Range},
    construction,
    plot::{self, Content, Legend, RetainedPlot, Style},
};
use agentelier_agent_host::{AgentHostPlugin, EcsError, SceneDefinition};
use agentelier_markdown::{MarkdownSource, MarkdownView};
use bevy::{prelude::*, ui::UiSystems};
use bevy_widgetry::scene::spawn_scene;
use serde_json::json;
use std::{collections::BTreeMap, io};

const DEFAULT_STEPS: usize = 352;

trait Model: Send + Sync + 'static {
    fn projection(&self, steps: usize) -> (Content, String);
    fn variables_mut(&mut self) -> &mut Variables;
    fn style(&self) -> Style;
    fn formula_name(&self) -> &'static str;
}

#[derive(Component)]
struct MathPlot {
    model: Box<dyn Model>,
    formula: Entity,
    steps: usize,
}

#[derive(Component)]
struct MathViewport(Entity);

struct Variables {
    names: Vec<String>,
    values: Vec<f64>,
    coordinates: usize,
}

impl Variables {
    fn new(coordinates: &[&str], parameters: BTreeMap<String, f64>) -> Result<Self> {
        if parameters.len() > 16
            || parameters.iter().any(|(name, &value)| {
                coordinates.contains(&name.as_str())
                    || !expression::valid_parameter(name)
                    || !value.is_finite()
            })
        {
            return Err(io::Error::other(
                "图形参数需使用有效的参数名及有限数值，最多 16 个，且不能同坐标变量重名",
            )
            .into());
        }
        let names = coordinates
            .iter()
            .map(|name| (*name).to_owned())
            .chain(parameters.keys().cloned())
            .collect();
        let values = std::iter::repeat_n(0.0, coordinates.len())
            .chain(parameters.values().copied())
            .collect();
        Ok(Self {
            names,
            values,
            coordinates: coordinates.len(),
        })
    }

    fn tex(&self) -> Vec<Option<f64>> {
        std::iter::repeat_n(None, self.coordinates)
            .chain(self.values[self.coordinates..].iter().copied().map(Some))
            .collect()
    }

    fn set(&mut self, variable: &str, value: f64) -> Result<bool> {
        let index = self.names[self.coordinates..]
            .iter()
            .position(|name| name == variable)
            .map(|index| index + self.coordinates)
            .ok_or_else(|| io::Error::other(format!("绘图没有参数 {variable}")))?;
        if self.values[index] == value {
            return Ok(false);
        }
        self.values[index] = value;
        Ok(true)
    }
}

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    let host = function::register(host)?;
    let host = implicit::register(host)?;
    let host = region::register(host)?;
    let host = polar::register(host)?;
    parametric::register(host)
}

pub(super) fn install(app: &mut App) {
    app.add_systems(PostUpdate, resample_viewports.after(UiSystems::Layout));
}

pub(in crate::agent_scenes) fn supports(template: &str) -> bool {
    matches!(
        template,
        function::TEMPLATE
            | implicit::TEMPLATE
            | region::TEMPLATE
            | polar::TEMPLATE
            | parametric::TEMPLATE
    )
}

pub(in crate::agent_scenes) fn bind_variable(
    scene: &mut SceneDefinition,
    variable: &str,
    value: f64,
) -> Result {
    let values = scene
        .props
        .get_mut("parameters")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or_else(|| io::Error::other("数学图形必须先声明 parameters"))?;
    let target = values
        .get_mut(variable)
        .ok_or_else(|| io::Error::other(format!("数学图形没有参数 {variable}")))?;
    *target = json!(value);
    Ok(())
}

pub(in crate::agent_scenes) fn set_variable(
    world: &mut World,
    root: Entity,
    variable: &str,
    value: f64,
) -> Result {
    let mut state = world
        .get_mut::<MathPlot>(root)
        .ok_or_else(|| io::Error::other("数学图形缺少当前变量"))?;
    if state.model.variables_mut().set(variable, value)? {
        refresh(world, root)?;
    }
    Ok(())
}

fn mount(
    world: &mut World,
    parent: Entity,
    model: impl Model,
) -> std::result::Result<Entity, EcsError> {
    let (content, source) = model.projection(DEFAULT_STEPS);
    let root = RetainedPlot::build(world, parent, content, model.style())?;
    let result =
        (|| {
            let viewport =
                plot::viewport_in(world, root).ok_or_else(|| construction("数学图形缺少视口"))?;
            let name = model.formula_name();
            let formula = spawn_scene(world, bsn! {
            ChildOf(root) @MarkdownView { @source: source, @source_controls: false } Name(name)
        }).map_err(construction)?;
            world.entity_mut(root).insert(MathPlot {
                model: Box::new(model),
                formula,
                steps: DEFAULT_STEPS,
            });
            world.entity_mut(viewport).insert(MathViewport(root));
            Ok(root)
        })();
    if result.is_err() {
        world.despawn(root);
    }
    result
}

fn update(world: &mut World, root: Entity, model: impl Model) -> Result<bool> {
    let old = instance(world, root)?;
    let (steps, formula) = (old.steps, old.formula);
    let (content, source) = model.projection(steps);
    if !RetainedPlot::update(world, root, &content)? {
        return Ok(false);
    }
    set_formula(world, formula, source)?;
    world.entity_mut(root).insert(MathPlot {
        model: Box::new(model),
        formula,
        steps,
    });
    Ok(true)
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

fn instance(world: &World, root: Entity) -> Result<&MathPlot> {
    world
        .get::<MathPlot>(root)
        .ok_or_else(|| io::Error::other("数学图形缺少表达式状态").into())
}

fn set_formula(world: &mut World, formula: Entity, source: String) -> Result {
    let mut target = world
        .get_mut::<MarkdownSource>(formula)
        .ok_or_else(|| io::Error::other("数学图形缺少公式区域"))?;
    target.set_if_neq(MarkdownSource(source));
    Ok(())
}

fn refresh(world: &mut World, root: Entity) -> Result {
    let state = instance(world, root)?;
    let (content, source) = state.model.projection(state.steps);
    let formula = state.formula;
    if !RetainedPlot::update(world, root, &content)? {
        return Err(io::Error::other("数学更新期间绘图区结构已变化").into());
    }
    set_formula(world, formula, source)
}

fn resample_viewports(
    views: Query<(&MathViewport, &ComputedNode), Changed<ComputedNode>>,
    mut commands: Commands,
) {
    for (view, layout) in &views {
        if layout.size().x <= 0.0 {
            continue;
        }
        let root = view.0;
        let steps = ((layout.size().x as usize / 3 / 32) * 32).clamp(128, 640);
        commands.queue(move |world: &mut World| -> Result {
            let Some(mut state) = world.get_mut::<MathPlot>(root) else {
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
