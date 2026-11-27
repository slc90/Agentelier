use crate::codex_socket;
use crate::model::{
    ModelFailure, ModelFailureKind, acquire, codex_tool_contract, http_failure, network, recover,
    request_id, responses, with_response,
};
use crate::protocol::ProtocolHistory;
use crate::stream::{GenerationResult, STREAM_IDLE_TIMEOUT, StreamEvent, consume};
use agentelier_account::{AuthenticatedRequest, CredentialClient, ExecutionIdentity};
use agentelier_prompts::InstructionSnapshot;
use agentelier_tools::{ToolCatalog, ToolContract, ToolMode};
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

pub(super) const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_HTTP_ATTEMPTS: u32 = 4;
const MAX_STREAM_RETRIES: u32 = 5;
const RETRY_BASE_DELAY_MS: u64 = 200;

pub struct GenerationRequest {
    pub behavior: Option<String>,
    pub expression: Option<crate::ExpressionContext>,
    pub memory: Option<crate::MemorySnapshot>,
    pub retired_memory_outputs: Vec<String>,
    pub transcript: Option<crate::TranscriptEvidence>,
    pub selected_skills: Vec<agentelier_skills::SelectedSkill>,
    pub mcp_materials: Vec<McpMaterial>,
    pub skill_catalog: Option<String>,
    pub slug: String,
    pub compact_threshold: u64,
    pub skip_compaction: bool,
    pub effort: Option<String>,
    pub fast: bool,
    pub context: ProtocolHistory,
    pub input_images: HashMap<(uuid::Uuid, uuid::Uuid), Arc<agentelier_tools::ImagePayload>>,
    pub history_position: usize,
    pub instructions: Arc<InstructionSnapshot>,
    pub tools: Arc<ToolCatalog>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordedModelSource {
    #[default]
    Siwc,
    Codex,
}

impl RecordedModelSource {
    fn is_siwc(&self) -> bool {
        *self == Self::Siwc
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct RequestProjection {
    #[serde(default, skip_serializing_if = "RecordedModelSource::is_siwc")]
    pub route: RecordedModelSource,
    #[serde(default)]
    pub fast: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction: Option<RequestCompaction>,
    #[serde(default, skip_serializing_if = "ToolContract::is_flat")]
    pub tool_contract: ToolContract,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behavior: Option<BehaviorMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<crate::ExpressionMaterial>,
    #[serde(default)]
    pub skill_materials: Vec<SkillMaterial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_materials: Vec<McpMaterial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory: Option<crate::MemoryMaterial>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub retired_memory_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub omitted_image_outputs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub user_images: Vec<crate::UserImage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript: Option<crate::TranscriptMaterial>,
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
pub struct McpMaterial {
    pub server: uuid::Uuid,
    pub target: String,
    pub tool: Option<String>,
    pub declared_names: Vec<String>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct BehaviorMaterial {
    pub input_index: usize,
    pub text: String,
    pub hash: String,
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
    body: bytes::Bytes,
    projection: RequestProjection,
    tools: Arc<ToolCatalog>,
}

#[derive(Default)]
pub struct GenerationConnection(Option<codex_socket::Continuation>);

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct SkillMaterial {
    #[serde(default)]
    pub path: Option<std::path::PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedded: Option<agentelier_skills::EmbeddedSkill>,
    pub role: String,
    pub input_index: usize,
    pub text: String,
    pub hash: String,
}

impl SkillMaterial {
    pub fn is_selected(&self) -> bool {
        self.path.is_some() || self.embedded.is_some()
    }
}

impl RequestProjection {
    pub fn additional_tools_in_input(&self) -> bool {
        self.tool_contract == ToolContract::AdditionalTools
    }

    pub fn validate_request_materials(&self) -> Result<(), ModelFailure> {
        use sha2::{Digest, Sha256};
        if self.tool_contract == ToolContract::CodexWebSocket
            && self.route != RecordedModelSource::Codex
        {
            return Err(crate::model::protocol(
                "Codex WebSocket 请求缺少 Codex 授权来源",
            ));
        }
        if self
            .fixed_bytes
            .is_some_and(|fixed| fixed > self.payload_bytes)
        {
            return Err(crate::model::protocol("冻结请求的固定材料超出请求大小"));
        }
        let mut retired = std::collections::HashSet::new();
        for retired_id in &self.retired_memory_outputs {
            if !retired.insert(retired_id) || !self.items.iter().any(|item| {
                matches!(item, crate::ProtocolItemSummary::FunctionCall { call_id, name }
                    if call_id == retired_id && crate::protocol::memory_read_tool(name))
            }) || !self.items.iter().any(|item| {
                matches!(item, crate::ProtocolItemSummary::FunctionResult { call_id } if call_id == retired_id)
            }) {
                return Err(crate::model::protocol("已撤下记忆结果缺少对应的工具调用"));
            }
        }
        if !self.omitted_image_outputs.is_empty() {
            let results: HashSet<_> = self
                .items
                .iter()
                .filter_map(|item| match item {
                    crate::ProtocolItemSummary::FunctionResult { call_id } => Some(call_id),
                    _ => None,
                })
                .collect();
            let mut omitted = HashSet::new();
            for id in &self.omitted_image_outputs {
                if !omitted.insert(id) || !results.contains(id) {
                    return Err(crate::model::protocol("省略的图片结果缺少对应的工具回执"));
                }
            }
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
        let behavior_index = usize::from(self.additional_tools_in_input()) + self.base_hashes.len();
        if let Some(behavior) = &self.behavior
            && (behavior.input_index != behavior_index
                || behavior.text.trim().is_empty()
                || behavior.hash != agentelier_prompts::content_hash(&behavior.text))
        {
            return Err(crate::model::protocol("任务情境材料位置或摘要无效"));
        }
        let expression_index = behavior_index + usize::from(self.behavior.is_some());
        if let Some(expression) = &self.expression {
            expression.validate(expression_index)?;
        }
        let memory_index = expression_index + usize::from(self.expression.is_some());
        let transcript_index = memory_index + usize::from(self.memory.is_some());
        let catalog_index = transcript_index + usize::from(self.transcript.is_some());
        if let Some(memory) = &self.memory {
            memory.validate(memory_index)?;
        }
        if let Some(transcript) = &self.transcript {
            transcript.validate(transcript_index)?;
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
                || (index != 0) != material.is_selected()
                || material.path.is_some() && material.embedded.is_some()
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
    if contract == ToolContract::CodexWebSocket && codex_tool_contract(&generation.slug) != contract
    {
        return Err(crate::model::protocol(
            "冻结的 Codex WebSocket 契约与模型路径不一致",
        ));
    }
    match contract {
        ToolContract::Flat | ToolContract::CodexWebSocket => {}
        ToolContract::AdditionalTools | ToolContract::ExperimentalNamespace => {
            return Err(crate::model::protocol("已停用的工具契约仅可查看历史"));
        }
    }
    let mut input = Vec::new();
    input.extend(
        generation
            .instructions
            .messages()
            .into_iter()
            .map(|message| serde_json::json!({"role": message.role, "content": message.text})),
    );
    let behavior = generation.behavior.map(|text| {
        let material = BehaviorMaterial {
            input_index: input.len(),
            hash: agentelier_prompts::content_hash(&text),
            text,
        };
        input.push(serde_json::json!({ "role": "developer", "content": material.text }));
        material
    });
    let expression = generation
        .expression
        .map(|context| {
            let material = crate::ExpressionMaterial::new(context, input.len())?;
            input.push(serde_json::json!({ "role": "developer", "content": material.text }));
            Ok::<_, ModelFailure>(material)
        })
        .transpose()?;
    let memory = generation
        .memory
        .map(|snapshot| {
            let material = crate::MemoryMaterial::new(snapshot, input.len());
            material.validate(input.len())?;
            input.push(serde_json::json!({"role": material.role, "content": material.text}));
            Ok::<_, ModelFailure>(material)
        })
        .transpose()?;
    let transcript = generation
        .transcript
        .map(|evidence| evidence.insert(&mut input));
    let mut skill_materials = Vec::new();
    if let Some(catalog) = generation.skill_catalog {
        use sha2::{Digest, Sha256};
        skill_materials.push(SkillMaterial {
            path: None,
            embedded: None,
            role: "developer".into(),
            input_index: input.len(),
            hash: format!("{:x}", Sha256::digest(catalog.as_bytes())),
            text: catalog.clone(),
        });
        input.push(serde_json::json!({"role":"developer", "content":catalog}));
    }
    if !generation.mcp_materials.is_empty() {
        let names = generation
            .mcp_materials
            .iter()
            .flat_map(|material| material.declared_names.iter().map(String::as_str))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
            .join(", ");
        input.push(serde_json::json!({"role":"developer","content":format!("The user explicitly referred to these MCP capabilities for this turn: {names}. Use them where relevant within the existing tool permissions.")}));
    }
    let user_images = generation.context.user_images().cloned().collect();
    input.extend(generation.context.into_input(&generation.input_images)?);
    for skill in generation.selected_skills {
        skill
            .validate()
            .map_err(|failure| crate::model::protocol(&failure.to_string()))?;
        let text = skill.render();
        use sha2::{Digest, Sha256};
        skill_materials.push(SkillMaterial {
            path: skill.skill.resource.path().map(std::path::Path::to_owned),
            embedded: skill.skill.resource.embedded(),
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
    payload["tools"] = tool_declarations(&generation.tools);
    payload["tool_choice"] = serde_json::json!("auto");
    payload["parallel_tool_calls"] = serde_json::json!(true);
    if let Some(effort) = &generation.effort {
        payload["reasoning"] = serde_json::json!({"effort": effort});
    }
    payload["service_tier"] = serde_json::json!(if generation.fast {
        "priority"
    } else {
        "default"
    });
    payload["instructions"] = serde_json::Value::String(String::new());
    if contract == ToolContract::CodexWebSocket {
        payload["type"] = serde_json::json!("response.create");
    }
    let body = serde_json::to_vec(&payload)
        .map_err(|error| crate::model::protocol(&format!("模型请求序列化失败：{error}")))?;
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
        route: RecordedModelSource::Codex,
        fast: generation.fast,
        compaction: Some(compaction),
        tool_contract: contract,
        behavior,
        expression,
        skill_materials,
        mcp_materials: generation.mcp_materials,
        memory,
        retired_memory_outputs: generation.retired_memory_outputs,
        omitted_image_outputs: Vec::new(),
        user_images,
        transcript,
        history: Some(history),
        items,
        base_hashes,
        payload_hash: format!("{:x}", Sha256::digest(&body)),
        payload_bytes: body.len(),
        fixed_bytes: None,
    };
    Ok(PreparedGeneration {
        body: body.into(),
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
                "parameters": tool.model_parameters(),
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
    connection: GenerationConnection,
    credentials: CredentialClient,
    generation: PreparedGeneration,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
) -> Result<(GenerationResult, GenerationConnection), ModelFailure> {
    let websocket = generation.projection.tool_contract == ToolContract::CodexWebSocket;
    let request = async {
        generate_inner(
            connection,
            credentials,
            generation,
            identity,
            progress,
            websocket,
        )
        .await
    };
    if websocket {
        tokio::select! { biased;
            _ = identity.invalidated() => Err(ModelFailure::new(
                ModelFailureKind::Authentication, "Codex 授权来源已断开，模型连接已停止"
            )),
            result = request => result,
        }
    } else {
        request.await
    }
}

async fn generate_inner(
    mut connection: GenerationConnection,
    credentials: CredentialClient,
    generation: PreparedGeneration,
    identity: &ExecutionIdentity,
    progress: Sender<StreamEvent>,
    websocket: bool,
) -> Result<(GenerationResult, GenerationConnection), ModelFailure> {
    let client = Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(STREAM_IDLE_TIMEOUT);
    let client = if websocket {
        client.http1_only()
    } else {
        client
    }
    .build()
    .map_err(|_| ModelFailure::new(ModelFailureKind::Network, "无法建立安全模型客户端"))?;
    let mut current = acquire(&credentials).await?;
    identity.validate(&current).map_err(|failure| {
        ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
    })?;
    tracing::debug!(
        request_bytes = generation.body.len(),
        payload_hash = generation.projection.payload_hash,
        tool_contract = ?generation.projection.tool_contract,
        "开始流式模型请求"
    );
    let mut request = AuthenticatedRequest::new(&current);
    let mut compressed = if websocket {
        None
    } else {
        let body = generation.body.clone();
        let original_bytes = body.len();
        let encoded =
            tokio::task::spawn_blocking(move || zstd::stream::encode_all(body.as_ref(), 3))
                .await
                .map_err(|_| crate::model::protocol("模型请求压缩工作异常结束"))?
                .map_err(|_| crate::model::protocol("无法压缩已冻结的模型请求"))?;
        tracing::debug!(
            original_bytes,
            compressed_bytes = encoded.len(),
            "冻结模型请求已压缩"
        );
        Some(bytes::Bytes::from(encoded))
    };
    let mut http_attempts = 1;
    let mut stream_retries = 0;
    loop {
        identity.validate(&current).map_err(|failure| {
            ModelFailure::new(ModelFailureKind::Authentication, failure.to_string())
        })?;
        progress
            .send(StreamEvent::Sending)
            .await
            .map_err(|_| crate::model::protocol("模型进度接收端已关闭"))?;
        let outcome = if websocket {
            codex_socket::generate(
                connection.0.take(),
                &client,
                &generation.body,
                &generation.tools,
                &current,
                identity,
                &mut request,
                &progress,
            )
            .await
            .map(|(result, continuation)| {
                connection.0 = Some(continuation);
                result
            })
            .map_err(FailedAttempt::Stream)
        } else {
            generate_http(
                &client,
                &generation,
                compressed.as_ref(),
                &current,
                &mut request,
                &progress,
            )
            .await
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
                return Ok((result, connection));
            }
            Err(attempt) => {
                let (failure, before_stream) = match attempt {
                    FailedAttempt::Http(failure) => (failure, true),
                    FailedAttempt::Stream(failure) => (failure, false),
                };
                let retryable_transport = matches!(failure.kind, ModelFailureKind::Network)
                    || failure
                        .status
                        .is_some_and(|status| (500..600).contains(&status));
                let use_plain_body = before_stream
                    && (failure.status == Some(415)
                        || failure.status == Some(400) && failure.code.is_none())
                    && compressed.take().is_some();
                let retry = if use_plain_body {
                    Some(1)
                } else if before_stream && retryable_transport && http_attempts < MAX_HTTP_ATTEMPTS
                {
                    let attempt = http_attempts;
                    http_attempts += 1;
                    Some(attempt)
                } else if (!before_stream && retryable_transport
                    || failure.code.as_deref() == Some("server_is_overloaded"))
                    && stream_retries < MAX_STREAM_RETRIES
                {
                    stream_retries += 1;
                    http_attempts = 1;
                    Some(stream_retries)
                } else {
                    None
                };
                if let Some(attempt) = retry {
                    let delay = Duration::from_millis(RETRY_BASE_DELAY_MS << (attempt - 1));
                    tracing::warn!(
                        request_id = failure.request_id.as_deref(),
                        kind = ?failure.kind,
                        status = failure.status,
                        http_attempts,
                        stream_retries,
                        ?delay,
                        "模型请求中断或服务暂不可用，等待后重试"
                    );
                    progress
                        .send(StreamEvent::Retrying)
                        .await
                        .map_err(|_| crate::model::protocol("模型进度接收端已关闭"))?;
                    request.reset_output_for_retry();
                    tokio::time::sleep(delay).await;
                } else if let Some(replacement) =
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

enum FailedAttempt {
    Http(ModelFailure),
    Stream(ModelFailure),
}

async fn generate_http(
    client: &Client,
    generation: &PreparedGeneration,
    compressed: Option<&bytes::Bytes>,
    current: &agentelier_account::UsableCredentials,
    request: &mut AuthenticatedRequest,
    progress: &Sender<StreamEvent>,
) -> Result<GenerationResult, FailedAttempt> {
    let sender = responses(client, current);
    let sender = if let Some(body) = compressed {
        sender
            .header(reqwest::header::CONTENT_ENCODING, "zstd")
            .body(body.clone())
    } else {
        sender.body(generation.body.clone())
    };
    let response = tokio::time::timeout(STREAM_IDLE_TIMEOUT, sender.send())
        .await
        .map_err(|_| {
            FailedAttempt::Http(ModelFailure::new(
                ModelFailureKind::Network,
                "等待模型响应超时",
            ))
        })?
        .map_err(|error| FailedAttempt::Http(network(error)))?;
    progress
        .send(StreamEvent::ResponseReceived {
            status: response.status().as_u16(),
        })
        .await
        .map_err(|_| FailedAttempt::Http(crate::model::protocol("模型进度接收端已关闭")))?;
    if response.status().is_success() {
        let status = response.status().as_u16();
        let id = request_id(&response);
        consume(response, request, Some(progress), Some(&generation.tools))
            .await
            .map_err(|failure| FailedAttempt::Stream(with_response(failure, status, id)))
    } else {
        let failure = http_failure(response).await;
        if let Some(usage) = failure.usage.clone() {
            progress
                .send(StreamEvent::Usage {
                    response_id: failure.response_id.clone(),
                    usage: *usage,
                })
                .await
                .map_err(|_| FailedAttempt::Http(crate::model::protocol("模型用量接收端已关闭")))?;
        }
        Err(FailedAttempt::Http(failure))
    }
}
