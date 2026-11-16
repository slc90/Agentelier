use crate::execution::ToolFailure;
use serde_json::{Value, json};
use std::collections::HashSet;

#[derive(Clone, Copy)]
pub(crate) enum Implementation {
    Exec,
    Stdin,
    Patch,
    Image { supported: Option<bool> },
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
    pub fn standard(
        directory: &std::path::Path,
        image_input: Option<bool>,
    ) -> Result<Self, ToolFailure> {
        let tools = vec![
            ToolDefinition {
                name: "view_image",
                description: format!("Read one existing local static PNG or JPEG and return its actual image content to the model, with safe metadata. Required JSON: {{\"path\":\"image.png\"}}. Relative paths resolve in fixed session directory {}; absolute paths use local Windows rules. Detects format from bytes, not extension. Does not capture screenshots, click, edit, generate or OCR images. At most 10 MiB input, 25 million pixels, longest sent edge 2048 px (proportional resize), 4 MiB encoded result, 16 MiB retained images per session and 32 MiB serialized model request. Immutable image bytes remain in history even if the original file changes or is deleted; use a new call to read new content. Model image capability from catalog: {}. Errors preserve existing history, without changing model or account.", directory.display(), match image_input { Some(true)=>"supported", Some(false)=>"unsupported", None=>"unknown; actual request determines support" }),
                parameters: json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false}),
                implementation: Implementation::Image {supported:image_input},
            },
            ToolDefinition {
                name: "apply_patch",
                description: format!("Modify UTF-8 text using a JSON object with required patch and workdir (string or null). Null workdir uses {}; relative workdir resolves there. Paths may be absolute or relative to workdir. Example: {{\"patch\":\"*** Begin Patch\\n*** Add File: hello.txt\\n+hello\\n*** End Patch\\n\",\"workdir\":null}}. Supports *** Add File:, *** Delete File:, *** Update File:, optional *** Move to: immediately after Update, @@ or @@ exact-anchor, context lines prefixed by space, - removals, + additions, and *** End of File after a chunk. Context must match exactly and uniquely. Read changed files before retrying. At most 64 actions, 8 MiB per file, 32 MiB prepared text. Preserves existing UTF-8 BOM and uniform LF/CRLF; new files use LF without BOM. Validates the whole patch before writing. Commits files separately, with actual action and temporary-file reports; errors or Stop preserve completed changes and never roll back. Does not overwrite existing add/move destinations.", directory.display()),
                parameters: json!({"type":"object","properties":{"patch":{"type":"string"},"workdir":{"type":["string","null"]}},"required":["patch","workdir"],"additionalProperties":false}),
                implementation: Implementation::Patch,
            },
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
