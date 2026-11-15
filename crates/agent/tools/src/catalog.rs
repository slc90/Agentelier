use crate::execution::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(crate) enum Implementation {
    Calculate,
    Wait,
}

#[derive(Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub parameters: Value,
    pub(crate) implementation: Implementation,
}

pub struct ToolCatalog(Vec<ToolDefinition>);
impl ToolCatalog {
    pub fn standard() -> Result<Self, ToolFailure> {
        let tools = vec![
            ToolDefinition {
                name: "diagnostic_calculate",
                description: "Calculate two finite numbers. Returns the result and a fresh execution receipt.",
                parameters: json!({"type":"object", "properties":{
                    "a":{"type":"number"}, "b":{"type":"number"},
                    "operation":{"type":"string","enum":["add","subtract","multiply","divide"]}
                }, "required":["a","b","operation"], "additionalProperties":false}),
                implementation: Implementation::Calculate,
            },
            ToolDefinition {
                name: "diagnostic_wait",
                description: "Wait asynchronously for 0 to 30000 milliseconds. Execution times out after 15 seconds. Returns elapsed time and a fresh receipt.",
                parameters: json!({"type":"object", "properties":{
                    "milliseconds":{"type":"integer", "minimum":0,"maximum":30000}
                }, "required":["milliseconds"], "additionalProperties":false}),
                implementation: Implementation::Wait,
            },
        ];
        let mut names = HashSet::new();
        for tool in &tools {
            if !names.insert(tool.name) {
                return Err(ToolFailure::new("duplicate_name", "工具目录含重复名称"));
            }
        }
        Ok(Self(tools))
    }

    pub fn definitions(&self) -> &[ToolDefinition] {
        &self.0
    }

    pub(crate) fn find(&self, name: &str) -> Option<Implementation> {
        self.0
            .iter()
            .find(|tool| tool.name == name)
            .map(|tool| tool.implementation)
    }
}
