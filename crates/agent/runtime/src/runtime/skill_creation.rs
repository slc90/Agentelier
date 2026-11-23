use agentelier_skills::{MetadataUpdate, ResourceDirectory, UserSkills};
use agentelier_tools::{
    ToolDefinition, ToolMode, ToolRegistration, ToolResult, ToolSource, ToolStatus,
};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy)]
enum Operation {
    Initialize,
    Metadata,
    Validate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Initialization {
    name: String,
    description: String,
    resources: Vec<ResourceDirectory>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    name: String,
    update: MetadataUpdate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Validation {
    name: String,
}

pub(super) fn registrations(store: UserSkills) -> impl Iterator<Item = ToolRegistration> {
    [
        Operation::Initialize,
        Operation::Metadata,
        Operation::Validate,
    ]
    .into_iter()
    .map(move |operation| operation.registration(store.clone()))
}

impl Operation {
    fn registration(self, store: UserSkills) -> ToolRegistration {
        let (name, description, parameters) = match self {
            Self::Initialize => (
                "initialize_user_skill",
                "Create a new Skill in the application user Skills directory. Normalize the name, write a starter SKILL.md, and create only the requested resource directories. The starter marker must be replaced with the authored workflow.",
                json!({
                    "type":"object","properties":{
                        "name":{"type":"string"},
                        "description":{"type":"string"},
                        "resources":{"type":"array","items":{"type":"string","enum":["references","scripts","assets"]}}
                    },
                    "required":["name","description","resources"],"additionalProperties":false
                }),
            ),
            Self::Metadata => (
                "write_skill_metadata",
                "Update only supplied agents/openai.yaml interface and policy fields for a user Skill. Use the SHA-256 returned by validate_user_skill, or null when the metadata file does not exist. Null interface fields retain their existing values.",
                metadata_schema(),
            ),
            Self::Validate => (
                "validate_user_skill",
                "Validate a user Skill's frontmatter, folder name, Markdown resource links and optional agents/openai.yaml. Returns the current metadata SHA-256 or null when absent.",
                json!({
                    "type":"object","properties":{"name":{"type":"string"}},
                    "required":["name"],"additionalProperties":false
                }),
            ),
        };
        let definition = ToolDefinition {
            name: name.into(),
            description: description.into(),
            parameters,
            source: ToolSource::Extension {
                provider: "agentelier.skills".into(),
                tool: name.into(),
            },
            strict: true,
            mode: ToolMode::Synchronous,
            display_source: None,
            output_schema: None,
        };
        let tool = ToolRegistration::new(definition, move |_, raw, cancel| {
            let store = store.clone();
            let arguments = raw.to_owned();
            let cancel = cancel.clone();
            Box::pin(async move {
                tokio::task::spawn_blocking(move || self.execute(&store, &arguments, &cancel))
                    .await
                    .unwrap_or_else(|failure| {
                        ToolResult::error(
                            ToolStatus::Unconfirmed,
                            "skill_worker_failed",
                            &failure.to_string(),
                        )
                    })
            })
        })
        .text_result();
        if matches!(self, Self::Validate) {
            tool.shared_read()
        } else {
            tool
        }
    }

    fn execute(self, store: &UserSkills, raw: &str, cancel: &CancellationToken) -> ToolResult {
        if cancel.is_cancelled() {
            return ToolResult::not_executed("skill_cancelled", "用户 Skill 操作已取消");
        }
        let result = match self {
            Self::Initialize => {
                let arguments: Initialization = match serde_json::from_str(raw) {
                    Ok(arguments) => arguments,
                    Err(failure) => return invalid(&failure),
                };
                store
                    .initialize(
                        &arguments.name,
                        &arguments.description,
                        &arguments.resources,
                        cancel,
                    )
                    .map(|receipt| json!(receipt))
            }
            Self::Metadata => {
                let arguments: Metadata = match serde_json::from_str(raw) {
                    Ok(arguments) => arguments,
                    Err(failure) => return invalid(&failure),
                };
                store
                    .write_metadata(&arguments.name, arguments.update, cancel)
                    .map(|sha| json!({"metadata_sha256": sha}))
            }
            Self::Validate => {
                let arguments: Validation = match serde_json::from_str(raw) {
                    Ok(arguments) => arguments,
                    Err(failure) => return invalid(&failure),
                };
                store
                    .validate(&arguments.name, cancel)
                    .map(|receipt| json!(receipt))
            }
        };
        match result {
            Ok(data) => ToolResult {
                status: ToolStatus::Succeeded,
                cleanup_failed: false,
                output: json!({"ok":true,"data":data}).to_string().into(),
            },
            Err(failure) => ToolResult::error(
                ToolStatus::Failed,
                "user_skill_operation",
                &failure.to_string(),
            ),
        }
    }
}

fn invalid(failure: &serde_json::Error) -> ToolResult {
    ToolResult::not_executed("invalid_user_skill_arguments", &failure.to_string())
}

fn metadata_schema() -> Value {
    json!({
        "type":"object","properties":{
            "name":{"type":"string"},
            "update":{
                "type":"object","properties":{
                    "expected_sha256":{"type":["string","null"]},
                    "interface":{
                        "type":"object","properties":{
                            "display_name":{"type":["string","null"]},
                            "short_description":{"type":["string","null"]},
                            "icon_small":{"type":["string","null"]},
                            "icon_large":{"type":["string","null"]},
                            "brand_color":{"type":["string","null"]},
                            "default_prompt":{"type":["string","null"]}
                        },
                        "required":["display_name","short_description","icon_small","icon_large","brand_color","default_prompt"],
                        "additionalProperties":false
                    },
                    "allow_implicit_invocation":{"type":["boolean","null"]}
                },
                "required":["expected_sha256","interface","allow_implicit_invocation"],
                "additionalProperties":false
            }
        },
        "required":["name","update"],"additionalProperties":false
    })
}
