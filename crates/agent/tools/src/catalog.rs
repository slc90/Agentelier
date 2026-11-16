use crate::execution::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(crate) enum Implementation {
    Diagnostic(crate::diagnostic::Diagnostic),
    Exec,
}

#[derive(Clone)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: String,
    pub parameters: Value,
    pub(crate) implementation: Implementation,
}

pub struct ToolCatalog(Vec<ToolDefinition>);
impl ToolCatalog {
    pub fn standard(directory: &std::path::Path) -> Result<Self, ToolFailure> {
        let tools = vec![
            ToolDefinition {
                name: "exec_command",
                description: format!("Execute a PowerShell 7 command with UTF-8 pipes, no profile, and a 15 second lifetime limit. workdir null uses the fixed session directory {}; relative workdir resolves there. Runs with current user permissions, without a sandbox. Returns stdout, stderr, actual exit code and truncation. There are no persistent sessions yet.", directory.display()),
                parameters: json!({"type":"object","properties":{
                    "cmd":{"type":"string"},"workdir":{"type":["string","null"]}
                },"required":["cmd","workdir"],"additionalProperties":false}),
                implementation: Implementation::Exec,
            },
            ToolDefinition {
                name: "diagnostic_calculate",
                description: "Calculate two finite numbers. Returns the result and a fresh execution receipt.".into(),
                parameters: json!({"type":"object", "properties":{
                    "a":{"type":"number"}, "b":{"type":"number"},
                    "operation":{"type":"string","enum":["add","subtract","multiply","divide"]}
                }, "required":["a","b","operation"], "additionalProperties":false}),
                implementation: Implementation::Diagnostic(crate::diagnostic::Diagnostic::Calculate),
            },
            ToolDefinition {
                name: "diagnostic_wait",
                description: "Wait asynchronously for 0 to 30000 milliseconds. Execution times out after 15 seconds. Returns elapsed time and a fresh receipt.".into(),
                parameters: json!({"type":"object", "properties":{
                    "milliseconds":{"type":"integer", "minimum":0,"maximum":30000}
                }, "required":["milliseconds"], "additionalProperties":false}),
                implementation: Implementation::Diagnostic(crate::diagnostic::Diagnostic::Wait),
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
