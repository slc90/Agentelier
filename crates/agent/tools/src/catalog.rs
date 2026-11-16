use crate::execution::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(crate) enum Implementation {
    Diagnostic(crate::diagnostic::Diagnostic),
    Exec,
    Stdin,
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
                description: format!("Execute PowerShell 7 using UTF-8 pipes and no profile. workdir null uses fixed session directory {}; relative values resolve there. yield_time_ms null waits 10000 ms; 0..30000 is allowed. If still running, returns a task-scoped session_id for write_stdin and new output. No command lifetime limit. At most 8 unreaped sessions. All processes are reaped when this user task ends; old IDs cannot be used in a later task. Runs with current-user access, without a sandbox or PTY.", directory.display()),
                parameters: json!({"type":"object","properties":{
                    "cmd":{"type":"string"},"workdir":{"type":["string","null"]},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000}
                },"required":["cmd","workdir","yield_time_ms"],"additionalProperties":false}),
                implementation: Implementation::Exec,
            },
            ToolDefinition {
                name: "write_stdin",
                description: "Continue a process session from this user task. Empty chars only polls for new output; nonempty chars writes exactly those UTF-8 bytes, without adding a newline. close_stdin closes input after this write (EOF). terminate kills and reaps that process group, and cannot be combined with nonempty chars or close_stdin. yield_time_ms null waits 10000 ms; range 0..30000. Output is consumed, never repeated; no automatic input retry. Nonzero exit codes are preserved. Sessions expire at task end. Pipes are not window keyboard input or a terminal.".into(),
                parameters: json!({"type":"object","properties":{
                    "session_id":{"type":"string"},"chars":{"type":"string"},
                    "yield_time_ms":{"type":["integer","null"],"minimum":0,"maximum":30000},
                    "close_stdin":{"type":"boolean"},"terminate":{"type":"boolean"}
                },"required":["session_id","chars","yield_time_ms","close_stdin","terminate"],"additionalProperties":false}),
                implementation: Implementation::Stdin,
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
