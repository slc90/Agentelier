use crate::model::{
    ModelFailure, ModelFailureKind, ModelRoute, acquire, http_failure, network, recover,
    request_id, with_response,
};
use crate::protocol::ProtocolHistory;
use crate::stream::{GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume};
use agentelier_account::{AuthenticatedRequest, CredentialClient, ExecutionIdentity};
use agentelier_prompts::InstructionSnapshot;
use agentelier_tools::{ToolCatalog, ToolContract, ToolMode};
use reqwest::Client;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;

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
    pub route: ModelRoute,
    pub memory: Option<crate::MemorySnapshot>,
    pub selected_skills: Vec<agentelier_skills::SelectedSkill>,
    pub skill_catalog: Option<String>,
    pub slug: String,
    pub compact_threshold: u64,
    pub skip_compaction: bool,
    pub reserved_result_bytes: usize,
    pub effort: Option<String>,
    pub context: ProtocolHistory,
    pub history_position: usize,
    pub instructions: Arc<InstructionSnapshot>,
    pub tools: Arc<ToolCatalog>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestProjection {
    #[serde(default, skip_serializing_if = "ModelRoute::is_siwc")]
    pub route: ModelRoute,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<RequestCompaction>,
    #[serde(default, skip_serializing_if = "ToolContract::is_flat")]
    pub tool_contract: ToolContract,
    #[serde(default)]
    pub skill_materials: Vec<SkillMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::MemoryMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<RequestHistory>,
    pub items: Vec<crate::ProtocolItemSummary>,
    pub base_hashes: Vec<String>,
    pub payload_hash: String,
    pub payload_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixed_bytes: Option<usize>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestHistory {
    pub position: usize,
    pub result_calls: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pending_calls: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RequestCompaction {
    Inline { threshold: u64 },
    PendingCalls,
    SkipOnce,
}

pub struct PreparedGeneration {
    body: Vec<u8>,
    projection: RequestProjection,
    tools: Arc<ToolCatalog>,
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
    pub fn additional_tools_in_input(&self) -> bool {
        self.tool_contract == ToolContract::AdditionalTools
    }

    pub fn validate_request_materials(&self) -> Result<(), ModelFailure> {
        use sha2::{Digest, Sha256};
        if self
            .fixed_bytes
            .is_some_and(|fixed| fixed > self.payload_bytes)
        {
            return Err(crate::model::protocol("冻结请求的固定材料超出请求大小"));
        }
        if let Some(history) = &self.history {
            let covered = self.items.len();
            let results = self.items.iter().filter_map(|item| match item {
                crate::ProtocolItemSummary::FunctionResult { call_id } => Some(call_id),
                _ => None,
            });
            if (history.position != covered
                && (history.position.checked_add(1) != Some(covered)
                    || !matches!(self.items.last(), Some(crate::ProtocolItemSummary::User))))
                || !results.eq(history.result_calls.iter())
            {
                return Err(crate::model::protocol("冻结请求的历史范围或结果集合不一致"));
            }
            let completed: HashSet<_> = history.result_calls.iter().collect();
            let pending = self.items.iter().filter_map(|item| match item {
                crate::ProtocolItemSummary::FunctionCall { call_id, .. }
                    if !completed.contains(call_id) =>
                {
                    Some(call_id)
                }
                _ => None,
            });
            if self.compaction.is_some()
                && (!pending.eq(&history.pending_calls)
                    || matches!(self.compaction, Some(RequestCompaction::Inline { .. }))
                        && !history.pending_calls.is_empty()
                    || self.compaction == Some(RequestCompaction::PendingCalls)
                        && history.pending_calls.is_empty())
            {
                return Err(crate::model::protocol("冻结请求的开放调用或压缩策略不一致"));
            }
        }
        let memory_index = usize::from(self.additional_tools_in_input()) + self.base_hashes.len();
        let catalog_index = memory_index + usize::from(self.memory.is_some());
        if let Some(memory) = &self.memory {
            memory.validate(memory_index)?;
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
    if generation.context.has_open_synchronous_calls() {
        return Err(crate::model::protocol("仍需收齐同步工具结果再发请求"));
    }
    let history = RequestHistory {
        position: generation.history_position,
        result_calls: generation.context.result_ids().map(str::to_owned).collect(),
        pending_calls: generation
            .context
            .open_calls()
            .map(|(id, _)| id.to_owned())
            .collect(),
    };
    let compaction = if generation.skip_compaction {
        RequestCompaction::SkipOnce
    } else if history.pending_calls.is_empty() {
        RequestCompaction::Inline {
            threshold: generation.compact_threshold,
        }
    } else {
        RequestCompaction::PendingCalls
    };
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
    let contract = generation.tools.contract();
    let declarations = tool_declarations(&generation.tools);
    let mut input = Vec::new();
    let top_level_tools = match contract {
        ToolContract::Flat => Some(declarations),
        ToolContract::AdditionalTools => {
            input.push(serde_json::json!({
                "type": "additional_tools",
                "role": "developer",
                "tools": declarations,
            }));
            None
        }
        ToolContract::ExperimentalNamespace => {
            return Err(crate::model::protocol(
                "已结束的 namespace 实验会话仅可查看",
            ));
        }
    };
    input.extend(
        generation
            .instructions
            .messages()
            .into_iter()
            .map(|message| serde_json::json!({"role": message.role, "content": message.text})),
    );
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
    let history_bytes = generation.context.input_wire_bytes()?;
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
        "input": input,
        "include": ["reasoning.encrypted_content"],
        "store": false,
        "stream": true,
    });
    if let RequestCompaction::Inline { threshold } = compaction {
        payload["context_management"] =
            serde_json::json!([{"type":"compaction", "compact_threshold":threshold}]);
    }
    if let Some(declarations) = top_level_tools {
        payload["tools"] = declarations;
    }
    payload["tool_choice"] = serde_json::json!("auto");
    payload["parallel_tool_calls"] = serde_json::json!(true);
    if let Some(effort) = &generation.effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    generation.route.responses_payload(&mut payload);
    let mut body = RequestBytes(Vec::new());
    serde_json::to_writer(&mut body, &payload).map_err(|error| {
        ModelFailure::new(
            ModelFailureKind::Protocol,
            format!("模型请求大小或序列化失败：{error}"),
        )
    })?;
    if generation.reserved_result_bytes > MAX_REQUEST_BYTES.saturating_sub(body.0.len()) {
        return Err(crate::model::protocol(
            "需保留待交付工具结果的请求空间，当前请求无法发送",
        ));
    }
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
        route: generation.route,
        compaction: Some(compaction),
        tool_contract: contract,
        skill_materials,
        memory,
        history: Some(history),
        items,
        base_hashes,
        payload_hash: format!("{:x}", Sha256::digest(&body.0)),
        payload_bytes: body.0.len(),
        fixed_bytes: Some(body.0.len().saturating_sub(history_bytes)),
    };
    Ok(PreparedGeneration {
        body: body.0,
        projection,
        tools: generation.tools,
    })
}

fn tool_declarations(catalog: &ToolCatalog) -> serde_json::Value {
    let declarations = catalog
        .definitions()
        .map(|tool| {
            let mut declaration = serde_json::json!({
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters,
                "strict": tool.strict,
            });
            if tool.mode == ToolMode::Asynchronous {
                declaration["async"] = serde_json::Value::Bool(true);
            }
            declaration
        })
        .collect();
    serde_json::Value::Array(declarations)
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
    if generation.projection.route != ModelRoute::of(&current) {
        return Err(ModelFailure::new(
            ModelFailureKind::Authentication,
            "当前授权来源与冻结请求不一致",
        ));
    }
    tracing::debug!(
        request_bytes = generation.body.len(),
        payload_hash = generation.projection.payload_hash,
        tool_contract = ?generation.projection.tool_contract,
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
            generation
                .projection
                .route
                .responses(&client, &current)
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
            consume(
                response,
                &mut request,
                Some(&progress),
                Some(&generation.tools),
            )
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
                identity.validate(&current).map_err(|failure| {
                    ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
                })?;
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
