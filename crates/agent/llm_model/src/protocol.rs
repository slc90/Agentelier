use agentelier_tools::ToolPayload;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
mod archive;
mod estimate;
pub use archive::{ArchivedPayload, ProtocolArchive};
pub use estimate::text_tokens as estimated_text_tokens;
pub use estimate::{result_token_budget, result_tokens, result_wire_bytes};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HistoryAppend {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProtocolItemSummary {
    Project {
        publication: u64,
        snapshot_hash: String,
    },
    Compaction,
    User,
    AgentResult,
    FunctionCall {
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
    },
    Assistant {
        phase: Option<String>,
    },
    Reasoning {
        encrypted: bool,
    },
}

#[derive(Clone)]
pub(crate) enum ProtocolItem {
    Project(crate::ProjectItem),
    Compaction(crate::CompactionItem),
    User(String),
    AgentResult(String),
    FunctionCall {
        raw: Value,
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
        output: ToolPayload,
    },
    Assistant {
        raw: Value,
        phase: Option<String>,
    },
    Reasoning {
        raw: Value,
        encrypted: bool,
    },
}

pub struct ProtocolOutput(pub(crate) Vec<ProtocolItem>);

impl ProtocolOutput {
    pub(crate) fn is_replayable(&self) -> bool {
        self.0.iter().all(|item| {
            !matches!(
                item,
                ProtocolItem::Reasoning {
                    encrypted: false,
                    ..
                }
            )
        })
    }
}

#[derive(Clone, Default)]
pub struct ProtocolHistory(Vec<ProtocolItem>);

impl ProtocolHistory {
    pub fn retire_memory_outputs(mut self, allowed: &HashSet<&str>) -> (Self, Vec<String>) {
        let retired = self
            .0
            .iter()
            .filter_map(|item| match item {
                ProtocolItem::FunctionCall { call_id, name, .. }
                    if memory_read_tool(name) && !allowed.contains(call_id.as_str()) =>
                {
                    Some(call_id.clone())
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut applied = Vec::new();
        for item in &mut self.0 {
            if let ProtocolItem::FunctionResult { call_id, output } = item
                && retired.contains(call_id)
            {
                *output = r#"{"ok":false,"error":{"code":"memory_view_expired","message":"Earlier derived memory is not part of the current turn's read view."}}"#.to_owned().into();
                applied.push(call_id.clone());
            }
        }
        (self, applied)
    }

    pub fn fork_at(&self, boundary: &[ProtocolItemSummary]) -> Result<Self, crate::ModelFailure> {
        let prefix = self
            .0
            .get(..boundary.len())
            .ok_or_else(|| crate::model::protocol("Fork 边界已不在当前协议历史"))?;
        let fork = Self(prefix.to_vec());
        if fork.summary() != boundary || fork.has_open_calls() {
            return Err(crate::model::protocol(
                "Fork 边界与冻结请求不符或存在未闭合的工具调用",
            ));
        }
        Ok(fork)
    }

    pub fn position(&self) -> usize {
        self.0.len()
    }

    pub(crate) fn result_ids(&self) -> impl Iterator<Item = &str> {
        self.0.iter().filter_map(|item| match item {
            ProtocolItem::FunctionResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
    }

    pub fn has_open_calls(&self) -> bool {
        !self.pending_calls().is_empty()
    }

    pub fn has_open_synchronous_calls(&self) -> bool {
        self.pending_calls()
            .values()
            .any(|asynchronous| !asynchronous)
    }

    pub fn open_calls(&self) -> impl Iterator<Item = (&str, &str)> {
        let pending = self.pending_calls();
        self.0.iter().filter_map(move |item| match item {
            ProtocolItem::FunctionCall { call_id, name, .. }
                if pending.contains_key(call_id.as_str()) =>
            {
                Some((call_id.as_str(), name.as_str()))
            }
            _ => None,
        })
    }

    pub fn accepts_compaction(&self, output: &ProtocolOutput) -> bool {
        let mut open = self.has_open_calls();
        for item in &output.0 {
            match item {
                ProtocolItem::FunctionCall { .. } => open = true,
                ProtocolItem::Compaction(_) if open => return false,
                _ => {}
            }
        }
        true
    }

    fn pending_calls(&self) -> HashMap<&str, bool> {
        let mut pending = HashMap::new();
        for item in &self.0 {
            match item {
                ProtocolItem::FunctionCall { call_id, raw, .. } => {
                    pending.insert(
                        call_id.as_str(),
                        raw.get("async").and_then(Value::as_bool) == Some(true),
                    );
                }
                ProtocolItem::FunctionResult { call_id, .. } => {
                    pending.remove(call_id.as_str());
                }
                _ => {}
            }
        }
        pending
    }

    pub fn with_project(&self, item: crate::ProjectItem) -> Self {
        let mut history = self.clone();
        history.0.push(ProtocolItem::Project(item));
        history
    }

    pub fn validate_projects(
        &self,
        publications: &[std::sync::Arc<crate::ProjectPublication>],
    ) -> Result<(), crate::ModelFailure> {
        for item in &self.0 {
            if let ProtocolItem::Project(item) = item
                && !publications.iter().any(|publication| {
                    publication.id == item.publication && publication.item.as_ref() == Some(item)
                })
            {
                return Err(crate::model::protocol("项目协议项缺少一致的来源发布"));
            }
        }
        Ok(())
    }
    pub fn compacted(&self) -> Self {
        Self(
            self.0[compaction_boundary(self.0.iter().map(|item| match item {
                ProtocolItem::FunctionCall { call_id, .. } => BoundaryItem::Call(call_id),
                ProtocolItem::FunctionResult { call_id, .. } => BoundaryItem::Result(call_id),
                ProtocolItem::Compaction(_) => BoundaryItem::Compaction,
                _ => BoundaryItem::Other,
            }))..]
                .to_vec(),
        )
    }

    pub fn replace_prefix(
        &self,
        covered: usize,
        replacement: &Self,
    ) -> Result<Self, crate::ModelFailure> {
        let tail = self
            .0
            .get(covered..)
            .ok_or_else(|| crate::model::protocol("压缩前缀超出工作历史"))?;
        Ok(Self(replacement.0.iter().chain(tail).cloned().collect()))
    }

    pub fn with_response(
        &self,
        input: Option<&str>,
        output: &ProtocolOutput,
    ) -> Result<Self, crate::ModelFailure> {
        self.response_append(input, output)?;
        let mut history = self.clone();
        if let Some(input) = input {
            history.0.push(ProtocolItem::User(input.to_owned()));
        }
        history.0.extend_from_slice(&output.0);
        Ok(history)
    }
    pub fn with_user(&self, input: &str) -> Self {
        let mut history = self.clone();
        history.0.push(ProtocolItem::User(input.to_owned()));
        history
    }

    pub fn with_agent_result(&self, input: &str) -> Self {
        let mut history = self.clone();
        history.0.push(ProtocolItem::AgentResult(input.to_owned()));
        history
    }

    pub fn response_append(
        &self,
        input: Option<&str>,
        output: &ProtocolOutput,
    ) -> Result<HistoryAppend, crate::ModelFailure> {
        if self
            .pending_calls()
            .values()
            .any(|asynchronous| !asynchronous)
        {
            return Err(crate::model::protocol("存在未配对的同步工具调用"));
        }
        let calls = self.0.iter().filter_map(|item| match item {
            ProtocolItem::FunctionCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        });
        let mut seen: HashSet<_> = calls.collect();
        for item in &output.0 {
            if let ProtocolItem::FunctionCall { call_id, .. } = item
                && !seen.insert(call_id)
            {
                return Err(crate::model::protocol("完整响应重用了历史调用身份"));
            }
        }
        self.next_append(output.len() + usize::from(input.is_some()))
    }

    pub fn append_response(
        &mut self,
        input: Option<&str>,
        output: ProtocolOutput,
    ) -> Result<HistoryAppend, crate::ModelFailure> {
        let append = self.response_append(input, &output)?;
        if let Some(input) = input {
            self.0.push(ProtocolItem::User(input.to_owned()));
        }
        self.0.extend(output.0);
        Ok(append)
    }

    pub fn result_append(&self, call_id: &str) -> Result<HistoryAppend, crate::ModelFailure> {
        match self.0.iter().rev().find(|item| match item {
            ProtocolItem::FunctionCall { call_id: id, .. }
            | ProtocolItem::FunctionResult { call_id: id, .. } => id == call_id,
            _ => false,
        }) {
            Some(ProtocolItem::FunctionCall { .. }) => self.next_append(1),
            Some(ProtocolItem::FunctionResult { .. }) => {
                Err(crate::model::protocol("工具结果已经追加"))
            }
            _ => Err(crate::model::protocol("工具结果缺少原始调用")),
        }
    }

    pub fn result_payload(&self, call_id: &str) -> Option<&ToolPayload> {
        self.0.iter().rev().find_map(|item| match item {
            ProtocolItem::FunctionResult {
                call_id: id,
                output,
            } if id == call_id => Some(output),
            _ => None,
        })
    }

    pub fn append_result(
        &mut self,
        call_id: String,
        output: ToolPayload,
    ) -> Result<HistoryAppend, crate::ModelFailure> {
        let append = self.result_append(&call_id)?;
        self.0
            .push(ProtocolItem::FunctionResult { call_id, output });
        Ok(append)
    }

    fn next_append(&self, count: usize) -> Result<HistoryAppend, crate::ModelFailure> {
        HistoryAppend::at(self.position(), count)
    }

    pub fn summary(&self) -> Vec<ProtocolItemSummary> {
        self.0
            .iter()
            .map(|item| match item {
                ProtocolItem::Project(item) => ProtocolItemSummary::Project {
                    publication: item.publication,
                    snapshot_hash: item.snapshot_hash.clone(),
                },
                ProtocolItem::User(_) => ProtocolItemSummary::User,
                ProtocolItem::AgentResult(_) => ProtocolItemSummary::AgentResult,
                ProtocolItem::Compaction(_) => ProtocolItemSummary::Compaction,
                ProtocolItem::FunctionCall { call_id, name, .. } => {
                    ProtocolItemSummary::FunctionCall {
                        call_id: call_id.clone(),
                        name: name.clone(),
                    }
                }
                ProtocolItem::FunctionResult { call_id, .. } => {
                    ProtocolItemSummary::FunctionResult {
                        call_id: call_id.clone(),
                    }
                }
                ProtocolItem::Assistant { phase, .. } => ProtocolItemSummary::Assistant {
                    phase: phase.clone(),
                },
                ProtocolItem::Reasoning { encrypted, .. } => ProtocolItemSummary::Reasoning {
                    encrypted: *encrypted,
                },
            })
            .collect()
    }

    pub fn image_bytes_with<'a>(&'a self, pending: impl Iterator<Item = &'a ToolPayload>) -> usize {
        let mut seen = std::collections::HashSet::new();
        self.0
            .iter()
            .filter_map(|item| match item {
                ProtocolItem::FunctionResult { output, .. } => output.image(),
                _ => None,
            })
            .chain(pending.filter_map(ToolPayload::image))
            .filter(|image| seen.insert(image.digest()))
            .map(|image| image.bytes().len())
            .sum()
    }

    pub(crate) fn into_input(self) -> Vec<Value> {
        self.0
            .into_iter()
            .map(|item| match item {
                ProtocolItem::Project(item) => serde_json::json!({"role":"user","content":item.text}),
                ProtocolItem::Compaction(item) => item.0,
                ProtocolItem::User(input) | ProtocolItem::AgentResult(input) => serde_json::json!({"role": "user", "content": input}),
                ProtocolItem::Assistant { raw, .. } | ProtocolItem::Reasoning { raw, .. } | ProtocolItem::FunctionCall {raw, ..} => raw,
                ProtocolItem::FunctionResult {call_id, output} => {
                    let body = match output {
                        ToolPayload::Text(text) => Value::String(text),
                        ToolPayload::GeneratedImage { .. } => Value::String(output.summary().into()),
                        ToolPayload::Image {summary,image} => {
                            use base64::Engine;
                            let url = format!("data:{};base64,{}", image.mime(), base64::engine::general_purpose::STANDARD.encode(image.bytes()));
                            let (width,height) = image.dimensions();
                            tracing::debug!(call_id, sha256=image.digest(), mime=image.mime(), width, height, encoded_bytes=image.bytes().len(), "图片工具结果正在转换为协议内容数组");
                            serde_json::json!([{"type":"input_text","text":summary},{"type":"input_image","image_url":url,"detail":"high"}])
                        }
                    };
                    serde_json::json!({"type":"function_call_output","call_id":call_id,"output":body})
                },
            })
            .collect()
    }
}

pub(crate) fn memory_read_tool(name: &str) -> bool {
    matches!(
        name,
        "search_memory" | "read_memory" | "read_memory_summary"
    )
}

enum BoundaryItem<'a> {
    Call(&'a str),
    Result(&'a str),
    Compaction,
    Other,
}

fn compaction_boundary<'a>(items: impl Iterator<Item = BoundaryItem<'a>>) -> usize {
    let mut pending = HashSet::new();
    let mut boundary = 0;
    for (index, item) in items.enumerate() {
        match item {
            BoundaryItem::Call(id) => {
                pending.insert(id);
            }
            BoundaryItem::Result(id) => {
                pending.remove(id);
            }
            BoundaryItem::Compaction if pending.is_empty() => boundary = index,
            _ => {}
        }
    }
    boundary
}

impl HistoryAppend {
    fn at(start: usize, count: usize) -> Result<Self, crate::ModelFailure> {
        let end = start
            .checked_add(count)
            .ok_or_else(|| crate::model::protocol("历史位置超出范围"))?;
        Ok(Self { start, end })
    }
}
