mod expression;
mod function;
mod trace;

use agentelier_agent_host::{AgentHostPlugin, SceneDefinition};
use bevy::prelude::*;

pub(in crate::agent_scenes) use function::TEMPLATE;

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    function::register(host)
}

pub(super) fn install(app: &mut App) {
    function::install(app);
}

pub(in crate::agent_scenes) fn bind_variable(
    scene: &mut SceneDefinition,
    variable: &str,
    value: f64,
) -> Result {
    function::bind_variable(scene, variable, value)
}

pub(in crate::agent_scenes) fn set_variable(
    world: &mut World,
    root: Entity,
    variable: &str,
    value: f64,
) -> Result {
    function::set_variable(world, root, variable, value)
}
