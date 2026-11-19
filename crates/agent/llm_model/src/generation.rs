use crate::model::{
    ModelFailure, ModelFailureKind, acquire, http_failure, network, recover, request_id,
    with_response,
};
use crate::protocol::ProtocolHistory;
use crate::stream::{GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume};
use agentelier_account::{AuthenticatedRequest, CredentialClient, ExecutionIdentity};
use agentelier_prompts::InstructionSnapshot;
use reqwest::Client;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

const RESPONSES_URL: &str = "https://api.openai.com/v1/responses";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

struct RequestBytes(Vec<u8>);
impl std::io::Write for RequestBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_REQUEST_BYTES.saturating_sub(self.0.len()) {
            return Err(std::io::Error::other(
                "完整模型请求超过 32 MiB，历史保持不变",
            ));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct GenerationRequest {
    pub memory: Option<crate::MemorySnapshot>,
    pub selected_skills: Vec<agentelier_skills::SelectedSkill>,
    pub skill_catalog: Option<String>,
    pub slug: String,
    pub compact_threshold: u64,
    pub effort: Option<String>,
    pub context: ProtocolHistory,
    pub instructions: Arc<InstructionSnapshot>,
    pub tools: Arc<agentelier_tools::ToolCatalog>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestProjection {
    #[serde(default)]
    pub skill_materials: Vec<SkillMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::MemoryMaterial>,
    pub items: Vec<crate::ProtocolItemSummary>,
    pub base_hashes: Vec<String>,
    pub payload_hash: String,
    pub payload_bytes: usize,
}

pub struct PreparedGeneration {
    body: Vec<u8>,
    projection: RequestProjection,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillMaterial {
    #[serde(default)]
    pub path: Option<std::path::PathBuf>,
    pub role: String,
    pub input_index: usize,
    pub text: String,
    pub hash: String,
}

impl RequestProjection {
    pub fn validate_request_materials(&self) -> Result<(), ModelFailure> {
        use sha2::{Digest, Sha256};
        let catalog_index = self.base_hashes.len() + usize::from(self.memory.is_some());
        if let Some(memory) = &self.memory {
            memory.validate(self.base_hashes.len())?;
        }
        let selected_start = self
            .skill_materials
            .get(1)
            .map_or(catalog_index + 1, |material| material.input_index);
        if selected_start != catalog_index + 1
            && selected_start != catalog_index + 1 + self.items.len()
        {
            return Err(crate::model::protocol(
                "选中 Skills 请求材料未处于完整历史边界",
            ));
        }
        for (index, material) in self.skill_materials.iter().enumerate() {
            let expected_index = if index == 0 {
                catalog_index
            } else {
                selected_start + index - 1
            };
            if material.input_index != expected_index
                || material.role != if index == 0 { "developer" } else { "user" }
                || (index == 0) != material.path.is_none()
                || material.hash != format!("{:x}", Sha256::digest(material.text.as_bytes()))
            {
                return Err(crate::model::protocol(
                    "Skills 请求材料角色、顺序或摘要无效",
                ));
            }
        }
        Ok(())
    }
}

impl PreparedGeneration {
    pub fn projection(&self) -> &RequestProjection {
        &self.projection
    }
}

pub fn prepare(generation: GenerationRequest) -> Result<PreparedGeneration, ModelFailure> {
    if generation.skill_catalog.is_none() && !generation.selected_skills.is_empty() {
        return Err(crate::model::protocol("显式 Skill 缺少任务目录"));
    }
    let items = generation.context.summary();
    let base_hashes = generation
        .instructions
        .messages()
        .iter()
        .map(|message| message.hash.clone())
        .collect();
    generation
        .instructions
        .validate()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Protocol, "会话指令快照无效"))?;
    let mut input: Vec<_> = generation
        .instructions
        .messages()
        .into_iter()
        .map(|message| serde_json::json!({"role": message.role, "content": message.text}))
        .collect();
    let memory = generation
        .memory
        .map(|snapshot| {
            let material = crate::MemoryMaterial::new(snapshot, input.len());
            material.validate(input.len())?;
            input.push(serde_json::json!({"role": material.role, "content": material.text}));
            Ok::<_, ModelFailure>(material)
        })
        .transpose()?;
    let mut skill_materials = Vec::new();
    if let Some(catalog) = generation.skill_catalog {
        use sha2::{Digest, Sha256};
        skill_materials.push(SkillMaterial {
            path: None,
            role: "developer".into(),
            input_index: input.len(),
            hash: format!("{:x}", Sha256::digest(catalog.as_bytes())),
            text: catalog.clone(),
        });
        input.push(serde_json::json!({"role":"developer", "content":catalog}));
    }
    input.extend(generation.context.into_input());
    for skill in generation.selected_skills {
        skill
            .validate()
            .map_err(|failure| crate::model::protocol(&failure.to_string()))?;
        let text = skill.render();
        use sha2::{Digest, Sha256};
        skill_materials.push(SkillMaterial {
            path: Some(skill.skill.path),
            role: "user".into(),
            input_index: input.len(),
            hash: format!("{:x}", Sha256::digest(text.as_bytes())),
            text: text.clone(),
        });
        input.push(serde_json::json!({"role":"user", "content":text}));
    }
    let mut payload = serde_json::json!({
        "model": generation.slug,
        "context_management": [{"type":"compaction", "compact_threshold":generation.compact_threshold}],
        "input": input,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
    });
    payload["tools"] = serde_json::Value::Array(generation.tools.definitions().map(|tool| serde_json::json!({
        "type":"function", "name":tool.name, "description":tool.description, "parameters":tool.parameters, "strict":tool.strict
    })).collect());
    payload["tool_choice"] = serde_json::json!("auto");
    payload["parallel_tool_calls"] = serde_json::json!(true);
    if let Some(effort) = &generation.effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    let mut body = RequestBytes(Vec::new());
    serde_json::to_writer(&mut body, &payload).map_err(|error| {
        ModelFailure::new(
            ModelFailureKind::Protocol,
            format!("模型请求大小或序列化失败：{error}"),
        )
    })?;
    for item in payload["input"].as_array().into_iter().flatten() {
        if let Some(content) = item.get("output").and_then(serde_json::Value::as_array) {
            for image in content.iter().filter(|part| part["type"] == "input_image") {
                tracing::debug!(
                    call_id = item["call_id"].as_str(),
                    output_kind = "content_array",
                    item_type = image["type"].as_str(),
                    detail = image["detail"].as_str(),
                    data_url = image["image_url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("data:image/")),
                    "请求中已包含真实图片内容项"
                );
            }
        }
    }
    use sha2::{Digest, Sha256};
    let projection = RequestProjection {
        skill_materials,
        memory,
        items,
        base_hashes,
        payload_hash: format!("{:x}", Sha256::digest(&body.0)),
        payload_bytes: body.0.len(),
    };
    Ok(PreparedGeneration {
        body: body.0,
        projection,
    })
}

pub async fn generate(
    credentials: CredentialClient,
    generation: PreparedGeneration,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
) -> Result<GenerationResult, ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_IDLE_TIMEOUT)
        .build()
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(&credentials).await?;
    identity.validate(&current).map_err(|failure| {
        ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
    })?;
    tracing::debug!(
        request_bytes = generation.body.len(),
        payload_hash = generation.projection.payload_hash,
        "开始流式模型请求"
    );
    let mut request = AuthenticatedRequest::new(&current);
    loop {
        identity.validate(&current).map_err(|failure| {
            ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
        })?;
        progress
            .send(StreamEvent::Sending)
            .await
            .map_err(|_| crate::model::protocol("模型进度接收端已关闭"))?;
        let response = tokio::time::timeout(
            STREAM_IDLE_TIMEOUT,
            client
                .post(RESPONSES_URL)
                .bearer_auth(current.access_token())
                .header(reqwest::header::ACCEPT, "text/event-stream")
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(generation.body.clone())
                .send(),
        )
        .await
        .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "等待模型响应超时"))?
        .map_err(network)?;
        progress
            .send(StreamEvent::ResponseReceived {
                status: response.status().as_u16(),
            })
            .await
            .map_err(|_| crate::model::protocol("模型进度接收端已关闭"))?;
        let outcome = if response.status().is_success() {
            let status = response.status().as_u16();
            let id = request_id(&response);
            consume(response, &mut request, Some(&progress), true)
                .await
                .map_err(|failure| with_response(failure, status, id))
        } else {
            let failure = http_failure(response).await;
            if let Some(usage) = failure.usage.clone() {
                progress
                    .send(StreamEvent::Usage {
                        response_id: failure.response_id.clone(),
                        usage: *usage,
                    })
                    .await
                    .map_err(|_| crate::model::protocol("模型用量接收端已关闭"))?;
            }
            Err(failure)
        };
        match outcome {
            Ok(result) => {
                if !result.output.is_replayable() {
                    return Err(ModelFailure::new(
                        ModelFailureKind::Protocol,
                        "完成响应缺少无状态续接所需的加密推理项",
                    ));
                }
                return Ok(result);
            }
            Err(failure) => {
                if let Some(replacement) =
                    recover(&credentials, &mut request, &current, &failure).await?
                {
                    current = replacement;
                } else {
                    return Err(failure);
                }
            }
        }
    }
}
