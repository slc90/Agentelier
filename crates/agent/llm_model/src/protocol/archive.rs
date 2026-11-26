use super::{HistoryAppend, ProtocolHistory, ProtocolItem, ProtocolOutput};
use agentelier_tools::{ImageDescriptor, ImagePayload, ToolPayload};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArchivedPayload {
    Text {
        text: String,
    },
    Image {
        summary: String,
        image: ImageDescriptor,
    },
}

impl ArchivedPayload {
    pub fn capture(payload: &ToolPayload) -> Self {
        match payload {
            ToolPayload::Text(text) => Self::Text { text: text.clone() },
            ToolPayload::GeneratedImage { .. }
            | ToolPayload::PublishAudio(_)
            | ToolPayload::PublishScene(_)
            | ToolPayload::UpdateScene(_) => Self::Text {
                text: payload.summary().into(),
            },
            ToolPayload::Image { summary, image } => Self::Image {
                summary: summary.clone(),
                image: image.descriptor(),
            },
        }
    }

    fn restore(
        self,
        load: &mut impl FnMut(&ImageDescriptor, &str) -> Result<Arc<ImagePayload>, crate::ModelFailure>,
    ) -> Result<ToolPayload, crate::ModelFailure> {
        Ok(match self {
            Self::Text { text } => ToolPayload::Text(text),
            Self::Image { summary, image } => {
                let payload = load(&image, &summary)?;
                if payload.descriptor() != image {
                    return Err(crate::model::protocol("恢复附件与协议引用不一致"));
                }
                ToolPayload::Image {
                    summary,
                    image: payload,
                }
            }
        })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum ArchivedItem {
    Project {
        item: crate::ProjectItem,
    },
    Compaction {
        item: crate::CompactionItem,
    },
    User {
        text: String,
    },
    AgentResult {
        text: String,
    },
    FunctionCall {
        raw: Value,
        call_id: String,
        name: String,
    },
    FunctionResult {
        call_id: String,
        output: ArchivedPayload,
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

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProtocolArchive {
    items: Vec<ArchivedItem>,
}

impl ProtocolArchive {
    pub fn appends_agent_result(&self, previous: &Self, user: Option<&str>, note: &str) -> bool {
        let Some((ArchivedItem::AgentResult { text }, prefix)) = self.items.split_last() else {
            return false;
        };
        text == note
            && prefix
                .strip_prefix(previous.items.as_slice())
                .is_some_and(|added| match (added, user) {
                    ([], None) => true,
                    ([ArchivedItem::User { text }], Some(user)) => text == user,
                    _ => false,
                })
    }

    pub fn appends_users(&self, previous: &Self, users: &[&str]) -> bool {
        self.items
            .strip_prefix(previous.items.as_slice())
            .is_some_and(|added| {
                added.len() == users.len() && added.iter().zip(users).all(|(item, expected)| {
                matches!(item, ArchivedItem::User { text } if text == expected)
            })
            })
    }

    pub fn prefix_fingerprint(&self, covered: usize) -> Result<String, crate::ModelFailure> {
        use sha2::{Digest, Sha256};

        struct DigestWriter(Sha256);
        impl std::io::Write for DigestWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.update(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let prefix = self
            .items
            .get(..covered)
            .ok_or_else(|| crate::model::protocol("压缩前缀超出存档历史"))?;
        let mut digest = DigestWriter(Sha256::new());
        serde_json::to_writer(&mut digest, prefix)
            .map_err(|failure| crate::model::protocol(&failure.to_string()))?;
        Ok(format!("{:x}", digest.0.finalize()))
    }

    pub fn replace_compacted_prefix(
        &self,
        covered: usize,
        fingerprint: &str,
        replacement: &Self,
    ) -> Result<Self, crate::ModelFailure> {
        use super::{BoundaryItem, compaction_boundary};
        if self.prefix_fingerprint(covered)? != fingerprint {
            return Err(crate::model::protocol("压缩所覆盖的历史前缀已变化"));
        }
        let prefix = &self.items[..covered];
        let cut = compaction_boundary(prefix.iter().map(|item| match item {
            ArchivedItem::FunctionCall { call_id, .. } => BoundaryItem::Call(call_id),
            ArchivedItem::FunctionResult { call_id, .. } => BoundaryItem::Result(call_id),
            ArchivedItem::Compaction { .. } => BoundaryItem::Compaction,
            _ => BoundaryItem::Other,
        }));
        if !matches!(prefix.get(cut), Some(ArchivedItem::Compaction { .. }))
            || prefix.get(cut..) != Some(replacement.items.as_slice())
        {
            return Err(crate::model::protocol(
                "替换窗口与原压缩边界或尾部事实不一致",
            ));
        }
        Ok(Self {
            items: replacement
                .items
                .iter()
                .chain(&self.items[covered..])
                .cloned()
                .collect(),
        })
    }

    pub fn accepts_compaction(&self, output: &Self) -> bool {
        let completed: HashSet<_> = self.result_ids().collect();
        let mut open = self.call_ids().any(|call| !completed.contains(call));
        for item in &output.items {
            match item {
                ArchivedItem::FunctionCall { .. } => open = true,
                ArchivedItem::Compaction { .. } if open => return false,
                _ => {}
            }
        }
        true
    }

    pub fn matches_request_history(
        &self,
        frozen: &crate::RequestHistory,
        check_pending: bool,
    ) -> bool {
        let Some(items) = self.items.get(..frozen.position) else {
            return false;
        };
        let results: Vec<_> = items
            .iter()
            .filter_map(|item| match item {
                ArchivedItem::FunctionResult { call_id, .. } => Some(call_id),
                _ => None,
            })
            .collect();
        if !results.iter().copied().eq(&frozen.result_calls) {
            return false;
        }
        if !check_pending {
            return true;
        }
        let completed: HashSet<_> = results.into_iter().collect();
        items
            .iter()
            .filter_map(|item| match item {
                ArchivedItem::FunctionCall { call_id, .. } if !completed.contains(call_id) => {
                    Some(call_id)
                }
                _ => None,
            })
            .eq(&frozen.pending_calls)
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn has_compaction(&self) -> bool {
        self.items
            .iter()
            .any(|item| matches!(item, ArchivedItem::Compaction { .. }))
    }
    pub fn assistant_ids(&self) -> Option<HashSet<&str>> {
        super::assistant_ids(self.items.iter().filter_map(|item| match item {
            ArchivedItem::Assistant { raw, .. } => Some(raw),
            _ => None,
        }))
    }
    pub fn validate_compaction(&self) -> Result<(), crate::ModelFailure> {
        if !self.has_compaction() {
            return Err(crate::model::protocol("压缩替换历史缺少压缩项"));
        }
        let mut pending = HashMap::new();
        let mut seen = HashSet::new();
        for item in &self.items {
            match item {
                ArchivedItem::FunctionCall { call_id, raw, .. } => {
                    if !seen.insert(call_id) {
                        return Err(crate::model::protocol("压缩历史调用身份重复"));
                    }
                    pending.insert(
                        call_id,
                        raw.get("async").and_then(Value::as_bool) == Some(true),
                    );
                }
                ArchivedItem::FunctionResult { call_id, .. } => {
                    if pending.remove(call_id).is_none() {
                        return Err(crate::model::protocol("压缩历史工具结果不匹配"));
                    }
                }
                ArchivedItem::User { .. }
                | ArchivedItem::AgentResult { .. }
                | ArchivedItem::Project { .. }
                    if pending.values().any(|asynchronous| !asynchronous) =>
                {
                    return Err(crate::model::protocol("压缩历史用户输入插入未封闭调用"));
                }
                _ => {}
            }
        }
        Ok(())
    }

    pub fn call_ids(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            ArchivedItem::FunctionCall { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
    }

    pub fn result_ids(&self) -> impl Iterator<Item = &str> {
        self.items.iter().filter_map(|item| match item {
            ArchivedItem::FunctionResult { call_id, .. } => Some(call_id.as_str()),
            _ => None,
        })
    }

    pub fn append_response(
        &mut self,
        input: Option<&str>,
        output: &ProtocolArchive,
        append: HistoryAppend,
    ) -> Result<(), crate::ModelFailure> {
        let finished: HashSet<_> = self.result_ids().collect();
        if self.items.iter().any(|item| {
            matches!(item,
                ArchivedItem::FunctionCall { raw, call_id, .. }
                    if !finished.contains(call_id.as_str())
                        && raw.get("async").and_then(Value::as_bool) != Some(true)
            )
        }) {
            return Err(crate::model::protocol("存档含有未配对的同步工具调用"));
        }
        let mut identities: HashSet<_> = self.call_ids().collect();
        if append != HistoryAppend::at(self.len(), output.len() + usize::from(input.is_some()))?
            || output.call_ids().any(|id| !identities.insert(id))
            || output.items.iter().any(|item| {
                matches!(
                    item,
                    ArchivedItem::User { .. }
                        | ArchivedItem::AgentResult { .. }
                        | ArchivedItem::Project { .. }
                        | ArchivedItem::FunctionResult { .. }
                )
            })
        {
            return Err(crate::model::protocol("已保存响应的身份或历史位置不一致"));
        }
        if let Some(text) = input {
            self.items.push(ArchivedItem::User { text: text.into() });
        }
        self.items.extend(output.items.iter().cloned());
        Ok(())
    }

    pub fn append_result(
        &mut self,
        call_id: String,
        output: ArchivedPayload,
        append: HistoryAppend,
    ) -> Result<(), crate::ModelFailure> {
        let matching = self.items.iter().rev().find(|item| match item {
            ArchivedItem::FunctionCall { call_id: id, .. }
            | ArchivedItem::FunctionResult { call_id: id, .. } => id == &call_id,
            _ => false,
        });
        if append != HistoryAppend::at(self.len(), 1)?
            || !matches!(matching, Some(ArchivedItem::FunctionCall { .. }))
        {
            return Err(crate::model::protocol("已保存结果的身份或历史位置不一致"));
        }
        self.items
            .push(ArchivedItem::FunctionResult { call_id, output });
        Ok(())
    }

    pub fn validate_tool_contract(
        &self,
        contract: agentelier_tools::ToolContract,
        tools: &[agentelier_tools::ToolDefinition],
    ) -> Result<(), crate::ModelFailure> {
        for call in self.function_calls() {
            let call = call?;
            call.validate_declaration(contract, tools.iter().find(|tool| tool.name == call.name))?;
        }
        Ok(())
    }

    pub fn function_calls(
        &self,
    ) -> impl Iterator<Item = Result<crate::FunctionCall, crate::ModelFailure>> {
        self.items.iter().enumerate().filter_map(|(index, item)| {
            let ArchivedItem::FunctionCall { raw, call_id, name } = item else {
                return None;
            };
            Some(crate::stream::function_call(raw, index).and_then(|call| {
                if raw.get("type").and_then(Value::as_str) != Some("function_call")
                    || &call.call_id != call_id
                    || &call.name != name
                {
                    return Err(crate::model::protocol("历史函数调用身份或协议不一致"));
                }
                Ok(call)
            }))
        })
    }

    pub fn seal_interrupted(
        &mut self,
        output: ProtocolArchive,
        mut results: std::collections::BTreeMap<String, ArchivedPayload>,
        started: &std::collections::HashSet<String>,
        input: Option<&str>,
    ) -> Result<(), crate::ModelFailure> {
        let calls = output.call_ids().map(str::to_owned).collect::<Vec<_>>();
        if calls.is_empty() {
            return Err(crate::model::protocol(
                "未完成的普通输出不能作为工具步骤恢复",
            ));
        }
        let unique = calls.iter().collect::<std::collections::HashSet<_>>();
        if unique.len() != calls.len()
            || results.keys().any(|id| !calls.contains(id))
            || started.iter().any(|id| !calls.contains(id))
            || output.items.iter().any(|item| {
                matches!(
                    item,
                    ArchivedItem::User { .. }
                        | ArchivedItem::AgentResult { .. }
                        | ArchivedItem::Project { .. }
                        | ArchivedItem::FunctionResult { .. }
                )
            })
        {
            return Err(crate::model::protocol("中断响应中的函数身份或结果不一致"));
        }
        if let Some(input) = input {
            self.items.push(ArchivedItem::User { text: input.into() });
        }
        self.items.extend(output.items);
        for call_id in calls {
            let output = results
                .remove(&call_id)
                .unwrap_or_else(|| ArchivedPayload::interrupted(started.contains(&call_id)));
            self.items
                .push(ArchivedItem::FunctionResult { call_id, output });
        }
        Ok(())
    }
}

impl ArchivedPayload {
    pub fn interrupted(started: bool) -> Self {
        let message = if started {
            "上次运行中断，执行结果未确认；不表示文件副作用已撤销"
        } else {
            "上次运行中断，此调用未执行"
        };
        Self::Text {
            text: serde_json::json!({"ok":false,"error":{"code":"aborted","message":message}})
                .to_string(),
        }
    }
}

fn capture(items: &[ProtocolItem]) -> ProtocolArchive {
    ProtocolArchive {
        items: items
            .iter()
            .map(|item| match item {
                ProtocolItem::Project(item) => ArchivedItem::Project { item: item.clone() },
                ProtocolItem::User(text) => ArchivedItem::User { text: text.clone() },
                ProtocolItem::AgentResult(text) => ArchivedItem::AgentResult { text: text.clone() },
                ProtocolItem::Compaction(item) => ArchivedItem::Compaction { item: item.clone() },
                ProtocolItem::FunctionCall { raw, call_id, name } => ArchivedItem::FunctionCall {
                    raw: raw.clone(),
                    call_id: call_id.clone(),
                    name: name.clone(),
                },
                ProtocolItem::FunctionResult { call_id, output } => ArchivedItem::FunctionResult {
                    call_id: call_id.clone(),
                    output: ArchivedPayload::capture(output),
                },
                ProtocolItem::Assistant { raw, phase } => ArchivedItem::Assistant {
                    raw: raw.clone(),
                    phase: phase.clone(),
                },
                ProtocolItem::Reasoning { raw, encrypted } => ArchivedItem::Reasoning {
                    raw: raw.clone(),
                    encrypted: *encrypted,
                },
            })
            .collect(),
    }
}

impl ProtocolHistory {
    pub fn archive(&self) -> ProtocolArchive {
        capture(&self.0)
    }

    pub fn from_archive(
        archive: ProtocolArchive,
        mut load: impl FnMut(&ImageDescriptor, &str) -> Result<Arc<ImagePayload>, crate::ModelFailure>,
    ) -> Result<Self, crate::ModelFailure> {
        let mut items = Vec::with_capacity(archive.items.len());
        let mut pending = HashMap::new();
        let mut seen = HashSet::new();
        for item in archive.items {
            let item = match item {
                ArchivedItem::Project { item } => {
                    if pending.values().any(|asynchronous| !asynchronous)
                        || item.publication == 0
                        || item.text.is_empty()
                    {
                        return Err(crate::model::protocol("项目协议项身份或边界无效"));
                    }
                    ProtocolItem::Project(item)
                }
                ArchivedItem::Compaction { item } => ProtocolItem::Compaction(item),
                ArchivedItem::User { text } => {
                    if pending.values().any(|asynchronous| !asynchronous) {
                        return Err(crate::model::protocol("历史用户消息插入未封闭同步调用"));
                    }
                    ProtocolItem::User(text)
                }
                ArchivedItem::AgentResult { text } => {
                    if text.is_empty() || pending.values().any(|asynchronous| !asynchronous) {
                        return Err(crate::model::protocol("历史协作结果缺少内容或边界无效"));
                    }
                    ProtocolItem::AgentResult(text)
                }
                ArchivedItem::FunctionCall { raw, call_id, name } => {
                    if call_id.is_empty()
                        || name.is_empty()
                        || raw.get("type").and_then(Value::as_str) != Some("function_call")
                        || raw.get("call_id").and_then(Value::as_str) != Some(call_id.as_str())
                        || raw.get("name").and_then(Value::as_str) != Some(name.as_str())
                        || !raw.get("arguments").is_some_and(Value::is_string)
                        || !seen.insert(call_id.clone())
                    {
                        return Err(crate::model::protocol("历史函数调用身份或协议不一致"));
                    }
                    pending.insert(
                        call_id.clone(),
                        raw.get("async").and_then(Value::as_bool).unwrap_or(false),
                    );
                    ProtocolItem::FunctionCall { raw, call_id, name }
                }
                ArchivedItem::FunctionResult { call_id, output } => {
                    if pending.remove(&call_id).is_none() {
                        return Err(crate::model::protocol("历史函数结果缺少唯一原始调用"));
                    }
                    ProtocolItem::FunctionResult {
                        call_id,
                        output: output.restore(&mut load)?,
                    }
                }
                ArchivedItem::Assistant { raw, phase } => {
                    if raw.get("type").and_then(Value::as_str) != Some("message")
                        || raw.get("role").and_then(Value::as_str) != Some("assistant")
                        || !raw.get("content").is_some_and(Value::is_array)
                        || raw.get("phase").and_then(Value::as_str) != phase.as_deref()
                    {
                        return Err(crate::model::protocol("历史助手协议项无效"));
                    }
                    ProtocolItem::Assistant { raw, phase }
                }
                ArchivedItem::Reasoning { raw, encrypted } => {
                    if raw.get("type").and_then(Value::as_str) != Some("reasoning")
                        || !encrypted
                        || !raw
                            .get("encrypted_content")
                            .and_then(Value::as_str)
                            .is_some_and(|text| !text.is_empty())
                    {
                        return Err(crate::model::protocol("历史 reasoning 项不可续接"));
                    }
                    ProtocolItem::Reasoning { raw, encrypted }
                }
            };
            items.push(item);
        }
        Ok(Self(items))
    }
}

impl ProtocolOutput {
    pub fn archive(&self) -> ProtocolArchive {
        capture(&self.0)
    }
}
