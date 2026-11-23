use agentelier_skills::{BUILTIN_NAME, EmbeddedSkill, READ_RESOURCE_TOOL, read_resource};
use agentelier_tools::{
    ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReadArguments {
    skill: EmbeddedSkill,
    path: String,
}

pub(super) fn registration() -> ToolRegistration {
    let definition = ToolDefinition {
        name: READ_RESOURCE_TOOL.into(),
        description: "Read one compiled-in Skill resource. Supply its embedded Skill identity and a forward-slash path relative to its root, such as SKILL.md or references/openai_yaml.md. Returns UTF-8 text or base64 image bytes. Does not access local files.".into(),
        parameters: json!({
            "type": "object",
            "properties": {
                "skill": {"type":"string","enum":[BUILTIN_NAME]},
                "path": {"type":"string"}
            },
            "required":["skill","path"],
            "additionalProperties":false
        }),
        source: ToolSource::Extension { provider: "agentelier.skills".into(), tool: READ_RESOURCE_TOOL.into() },
        strict: true,
        mode: ToolMode::Synchronous,
        display_source: None,
        output_schema: None,
    };
    ToolRegistration::new(definition, |_, raw, cancel| Box::pin(async move {
        let arguments = match serde_json::from_str::<ReadArguments>(raw) {
            Ok(arguments) => arguments,
            Err(failure) => return ToolResult::not_executed("invalid_skill_resource", &failure.to_string()),
        };
        let resource = match read_resource(arguments.skill, &arguments.path, cancel) {
            Ok(resource) => resource,
            Err(failure) => return ToolResult::error(ToolStatus::Failed, "skill_resource_unavailable", &failure.to_string()),
        };
        let mut data = json!({"skill": arguments.skill.name(), "path": arguments.path, "media_type": resource.media_type});
        match std::str::from_utf8(resource.bytes) {
            Ok(text) => data["text"] = json!(text),
            Err(_) => data["base64"] = json!(STANDARD.encode(resource.bytes)),
        }
        ToolResult {
            status: ToolStatus::Succeeded,
            cleanup_failed: false,
            output: json!({"ok":true,"data":data}).to_string().into(),
        }
    })).shared_read().text_result()
}
