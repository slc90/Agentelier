use crate::conversation::ChatSession;
use crate::{
    Conversation, ConversationFailure, InitialSession, RecordEvent, SessionRecord, SessionSettings,
    SessionView, StoredImage, StoredState, TurnStatus,
};
use agentelier_llm_model::{
    ArchivedPayload, ModelFailure, ModelFailureKind, ProtocolArchive, ProtocolHistory,
};
use agentelier_tools::{ImagePayload, MAX_SESSION_IMAGE_BYTES, ToolCatalog, ToolStatus};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub struct RestoredSession {
    pub state: StoredState,
    initial: InitialSession,
    history: ProtocolHistory,
    publications: Vec<Arc<agentelier_llm_model::ProjectPublication>>,
    next_turn: u64,
    notice: Option<String>,
    pub normalized: bool,
}

impl RestoredSession {
    pub fn tool_definitions(&self) -> &[crate::StoredTool] {
        &self.initial.settings.tools
    }

    pub fn add_recovery_notice(&mut self, notice: &str) {
        self.notice = Some(match self.notice.take() {
            Some(previous) => format!("{previous}\n{notice}"),
            None => notice.into(),
        });
    }
    pub fn recovery_event(&self) -> RecordEvent {
        RecordEvent::Recovered {
            state: self.state.clone(),
            history: self.history.archive(),
            notice: self
                .notice
                .clone()
                .unwrap_or_else(|| "已恢复有效历史，待办保持暂停".into()),
        }
    }
}

struct PendingResponse {
    output: ProtocolArchive,
    results: BTreeMap<String, ArchivedPayload>,
    started: HashSet<String>,
    request_index: u32,
}

fn model_failure(message: impl ToString) -> ModelFailure {
    ModelFailure {
        usage: None,
        response_id: None,
        kind: ModelFailureKind::Protocol,
        status: None,
        code: None,
        request_id: None,
        message: message.to_string(),
    }
}

fn maximum(state: &StoredState) -> u64 {
    state
        .previous_turns
        .iter()
        .chain(state.turn.iter().map(Box::as_ref))
        .flat_map(|turn| [turn.id.0, turn.input_id.0])
        .chain(state.queue.iter().map(|input| input.input_id.0))
        .max()
        .unwrap_or(0)
}

fn merge(state: &mut StoredState, mut incoming: StoredState) {
    // Runner snapshots may lag queue commits, which have their own replay path.
    incoming.queue = std::mem::take(&mut state.queue);
    incoming.queue_paused = state.queue_paused;
    *state = incoming;
}

pub fn restore(
    initial: InitialSession,
    records: Vec<SessionRecord>,
    mut load: impl FnMut(&StoredImage) -> Result<Arc<ImagePayload>, ConversationFailure>,
) -> Result<RestoredSession, ConversationFailure> {
    initial
        .settings
        .instructions
        .validate()
        .map_err(|failure| ConversationFailure::new(format!("存档指令无效：{failure}")))?;
    let mut state = StoredState {
        project_error: None,
        context: Box::default(),
        session: initial.session,
        turn: None,
        previous_turns: Vec::new(),
        committed_items: Vec::new(),
        execution_blocked: false,
        queue: Vec::new(),
        queue_paused: false,
    };
    let mut publications = Vec::<Arc<agentelier_llm_model::ProjectPublication>>::new();
    let mut history = ProtocolArchive::default();
    if let Some(memory) = &initial.settings.memory {
        memory
            .validate()
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    }
    let mut pending: Option<PendingResponse> = None;
    let mut retained_input = false;
    let mut covered_sequence = 1;
    let mut next_turn = 0;
    let mut notices = Vec::new();
    for record in records {
        if record.session != initial.session || record.event.state().session != initial.session {
            return Err(ConversationFailure::new("存档事件属于另一会话"));
        }
        next_turn = next_turn.max(maximum(record.event.state()));
        match record.event {
            RecordEvent::ProjectPublished {
                state: incoming,
                publication,
                history: published,
            } => {
                publication
                    .validate()
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                if pending.is_some()
                    || publication.id != publications.len() as u64 + 1
                    || publication.window != state.context.window
                    || publication.snapshot.working_directory != initial.settings.working_directory
                    || published.is_some() != publication.item.is_some()
                {
                    return Err(ConversationFailure::new("项目发布身份或历史边界不一致"));
                }
                if let Some(published) = published {
                    history = published;
                    covered_sequence = record.sequence;
                }
                publications.push(Arc::new(*publication));
                merge(&mut state, incoming);
            }
            RecordEvent::RequestPrepared { state: incoming }
            | RecordEvent::RequestProgress { state: incoming } => {
                let turn = incoming
                    .turn
                    .as_ref()
                    .ok_or_else(|| ConversationFailure::new("准备记录缺少任务"))?;
                let fact = turn
                    .requests
                    .last()
                    .ok_or_else(|| ConversationFailure::new("准备记录缺少请求材料"))?;
                fact.projection
                    .validate_request_materials()
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                for selected in &turn.selected_skills {
                    selected
                        .validate()
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                }
                let selected_materials: Vec<_> = fact
                    .projection
                    .skill_materials
                    .iter()
                    .filter(|material| material.path.is_some())
                    .collect();
                if selected_materials.len() != turn.selected_skills.len()
                    || selected_materials.iter().zip(&turn.selected_skills).any(
                        |(material, selected)| {
                            material.path.as_ref() != Some(&selected.skill.path)
                                || material.text != selected.render()
                        },
                    )
                {
                    return Err(ConversationFailure::new(
                        "请求选中 Skills 与任务正文快照不一致",
                    ));
                }
                if pending.is_some()
                    || fact.projection.memory.as_ref().is_some_and(|material| {
                        Some(&material.snapshot) != initial.settings.memory.as_ref()
                    })
                    || fact.request != turn.request_index
                    || fact.window != incoming.context.window
                    || fact.publication != publications.last().map(|publication| publication.id)
                    || fact.projection.base_hashes
                        != initial
                            .settings
                            .instructions
                            .messages()
                            .iter()
                            .map(|message| message.hash.clone())
                            .collect::<Vec<_>>()
                {
                    return Err(ConversationFailure::new("请求准备身份或材料引用不一致"));
                }
                merge(&mut state, incoming);
            }
            RecordEvent::Compacted {
                state: incoming,
                checkpoint,
            } => {
                checkpoint.validate(&incoming)?;
                if pending.is_some()
                    || checkpoint.project_publication
                        != publications.last().map(|publication| publication.id)
                    || publications
                        .last()
                        .is_some_and(|publication| publication.window > checkpoint.source_window)
                    || checkpoint.source_window != state.context.window
                    || checkpoint.source_sequence != covered_sequence
                    || checkpoint.source_sequence >= record.sequence
                    || checkpoint.covered_items != history.len()
                    || checkpoint.input_retained != retained_input
                    || checkpoint.model != initial.settings.slug
                    || state
                        .turn
                        .as_ref()
                        .is_none_or(|turn| turn.id != checkpoint.turn)
                {
                    return Err(ConversationFailure::new(
                        "检查点未覆盖当前封闭历史或活动任务",
                    ));
                }
                history = checkpoint.history;
                retained_input = checkpoint.input_retained;
                covered_sequence = record.sequence;
                merge(&mut state, incoming);
            }
            RecordEvent::Accepted { state: incoming } => {
                notices.clear();
                if pending.is_some() {
                    return Err(ConversationFailure::new("存档在未封闭响应之后开始另一任务"));
                }
                state = incoming;
                retained_input = false;
                covered_sequence = record.sequence;
            }
            RecordEvent::Response {
                state: incoming,
                output,
            } => {
                if pending.is_some() {
                    return Err(ConversationFailure::new("存档完整响应没有封闭边界"));
                }
                let turn = incoming
                    .turn
                    .as_ref()
                    .ok_or_else(|| ConversationFailure::new("完整响应缺少任务身份"))?;
                pending = Some(PendingResponse {
                    output,
                    results: BTreeMap::new(),
                    started: HashSet::new(),
                    request_index: turn.request_index,
                });
                merge(&mut state, incoming);
            }
            RecordEvent::ToolStarted {
                state: incoming,
                call_id,
            } => {
                let response = pending
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("工具开始缺少完整响应"))?;
                if !response.output.call_ids().any(|id| id == call_id)
                    || !response.started.insert(call_id)
                {
                    return Err(ConversationFailure::new("工具开始身份未知或重复"));
                }
                merge(&mut state, incoming);
            }
            RecordEvent::ToolReturned {
                state: incoming,
                call_id,
                status,
                output,
            } => {
                let response = pending
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("工具结果缺少完整响应"))?;
                if !response.output.call_ids().any(|id| id == call_id)
                    || (status != ToolStatus::NotExecuted && !response.started.contains(&call_id))
                    || response.results.insert(call_id, output).is_some()
                {
                    return Err(ConversationFailure::new(
                        "工具结果身份未知、重复或缺少开始事实",
                    ));
                }
                merge(&mut state, incoming);
            }
            RecordEvent::Sealed {
                state: incoming,
                history: complete,
            } => {
                let response = pending
                    .take()
                    .ok_or_else(|| ConversationFailure::new("封闭步骤缺少完整响应"))?;
                if response.output.call_ids().count() != response.results.len() {
                    return Err(ConversationFailure::new("封闭步骤缺少实际工具结果"));
                }
                history = complete;
                retained_input = true;
                covered_sequence = record.sequence;
                merge(&mut state, incoming);
            }
            RecordEvent::SteerAccepted { state: incoming } => merge(&mut state, incoming),
            RecordEvent::SteerApplied {
                state: incoming,
                history: applied,
            } => {
                if pending.is_some() {
                    return Err(ConversationFailure::new("补充输入应用于未封闭响应"));
                }
                history = applied;
                retained_input = true;
                covered_sequence = record.sequence;
                merge(&mut state, incoming);
            }
            RecordEvent::QueueSuspended { .. } => state.queue_paused = true,
            RecordEvent::QueueChanged {
                state: incoming,
                decision,
            } => {
                state.queue = incoming.queue;
                state.queue_paused = incoming.queue_paused
                    || (state.queue_paused && !matches!(decision, crate::QueueDecision::Resumed));
            }
            RecordEvent::Finalized {
                state: incoming,
                history: finalized,
            } => {
                history = finalized;
                state.queue_paused |= incoming.queue_paused;
                merge(&mut state, incoming);
            }
            RecordEvent::Finished {
                state: mut incoming,
                history: finished,
            } => {
                if let Some(finished) = finished {
                    history = finished;
                } else {
                    // An unconfirmed writer acknowledgement must not overwrite a durable replacement.
                    incoming.context = state.context.clone();
                    incoming.committed_items = state.committed_items.clone();
                }
                state.queue_paused |= incoming.queue_paused;
                merge(&mut state, incoming);
            }
            RecordEvent::Recovered {
                state: incoming,
                history: recovered,
                notice,
            } => {
                state = incoming;
                covered_sequence = record.sequence;
                history = recovered;
                pending = None;
                retained_input = state
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.committed_steps > 0);
                if !notice.is_empty() {
                    notices.push(notice);
                }
            }
        }
    }
    let mut normalized = false;
    if let Some(response) = pending {
        let calls = response
            .output
            .call_ids()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let turn = state
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("中断响应缺少任务"))?;
        if !calls.is_empty() {
            if response.output.has_compaction() {
                state.context.anchor = None;
            }
            for call_id in &calls {
                let tool = turn
                    .tools
                    .iter_mut()
                    .find(|tool| {
                        tool.request_index == response.request_index && tool.call_id == *call_id
                    })
                    .ok_or_else(|| ConversationFailure::new("中断响应缺少对应工具展示身份"))?;
                if let Some(output) = response.results.get(call_id) {
                    tool.output = Some(match output {
                        ArchivedPayload::Text { text } => text.clone(),
                        ArchivedPayload::Image { summary, .. } => summary.clone(),
                    });
                } else {
                    tool.status = if response.started.contains(call_id) {
                        ToolStatus::Unconfirmed
                    } else {
                        ToolStatus::NotExecuted
                    };
                    tool.output = Some(
                        if response.started.contains(call_id) {
                            "上次运行中断，执行结果未确认；不表示文件副作用已撤销"
                        } else {
                            "上次运行中断，此调用未执行"
                        }
                        .into(),
                    );
                }
                tool.retained = true;
            }
            history
                .seal_interrupted(
                    response.output,
                    response.results,
                    &response.started,
                    (!retained_input).then_some(turn.input.as_str()),
                )
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
            turn.committed_steps = turn
                .committed_steps
                .checked_add(1)
                .ok_or_else(|| ConversationFailure::new("恢复步骤数超出范围"))?;
        }
        turn.status =
            TurnStatus::Failed("上次运行中断；已保留实际记录，缺失结果仅作为中断说明续接".into());
        normalized = true;
        notices.push(
            if calls.is_empty() {
                "上次运行在提交前中断，已有展示保留，未完成输出没有进入工作上下文"
            } else {
                "上次运行中断，工具结果未确认的部分不会自动重试"
            }
            .into(),
        );
    }
    if let Some(turn) = &mut state.turn {
        if matches!(turn.status, TurnStatus::Running) {
            turn.status = TurnStatus::Failed("上次运行在响应完成前中断".into());
            normalized = true;
            notices.push("未完成的模型输出没有进入工作上下文".into());
        }
        for steer in &mut turn.steers {
            if matches!(steer.state, crate::SteerState::Accepted) {
                steer.state = crate::SteerState::NotApplied;
                normalized = true;
            }
        }
        for tool in &mut turn.tools {
            if matches!(tool.status, ToolStatus::Receiving | ToolStatus::Pending) {
                tool.status = ToolStatus::NotExecuted;
                tool.output = Some("模型响应未完成，此调用未执行".into());
                tool.retained = false;
                normalized = true;
            }
        }
    }
    state.queue_paused |= !state.queue.is_empty();
    state.execution_blocked = false;
    let mut images = HashMap::<String, Arc<ImagePayload>>::new();
    let history = ProtocolHistory::from_archive(history, |descriptor, summary| {
        if let Some(image) = images.get(&descriptor.digest) {
            if image.descriptor() != *descriptor {
                return Err(model_failure("相同图片摘要的描述符不一致"));
            }
            return Ok(image.clone());
        }
        let image = load(&StoredImage {
            session: initial.session,
            summary: summary.into(),
            image: descriptor.clone(),
        })
        .map_err(model_failure)?;
        images.insert(descriptor.digest.clone(), image.clone());
        Ok(image)
    })
    .map_err(|failure| ConversationFailure::new(format!("无法恢复模型历史：{failure}")))?;
    history
        .validate_projects(&publications)
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    if history.image_bytes_with(std::iter::empty()) > MAX_SESSION_IMAGE_BYTES {
        return Err(ConversationFailure::new("恢复的工作上下文超过会话图片预算"));
    }
    state.committed_items = history.summary();
    state.context.refresh(
        &history,
        &initial.settings.instructions,
        &initial.settings.tools,
    )?;
    Ok(RestoredSession {
        state,
        initial,
        history,
        publications,
        next_turn,
        notice: (!notices.is_empty()).then(|| notices.join("\n")),
        normalized,
    })
}

impl Conversation {
    pub fn install_restored(
        &mut self,
        mut restored: RestoredSession,
        tools: &[agentelier_tools::ToolRegistration],
    ) -> SessionView {
        for turn in restored
            .state
            .previous_turns
            .iter_mut()
            .chain(restored.state.turn.iter_mut().map(Box::as_mut))
        {
            turn.operation_id = 0;
        }
        let settings = &restored.initial.settings;
        let tools = ToolCatalog::with_extensions(
            &settings.working_directory,
            settings.image_input,
            self.extensions.iter().chain(tools),
        )
        .and_then(|available| available.rebind(&settings.tools));
        let tools = match tools {
            Ok(tools) if settings.working_directory.is_absolute() => {
                if let Err(failure) =
                    agentelier_tools::WorkingDirectory::validate(&settings.working_directory)
                {
                    restored.add_recovery_notice(&format!(
                        "原工作目录不可用，工具执行会拒绝该路径：{failure}"
                    ));
                }
                tools
            }
            result => {
                restored.state.execution_blocked = true;
                let reason = result.err().map_or_else(
                    || "存档工作目录不是绝对路径".into(),
                    |failure| failure.to_string(),
                );
                restored.add_recovery_notice(&format!("{reason}，已阻止执行；历史仍可查看"));
                ToolCatalog::default()
            }
        };

        let settings = restored.initial.settings;
        let view = SessionView {
            project_context: Box::new(match restored.state.project_error {
                Some(message) => crate::ProjectContextState::Failed {
                    message,
                    last_success: restored
                        .publications
                        .last()
                        .map(|publication| Arc::new(publication.snapshot.clone())),
                },
                None => restored.publications.last().map_or(
                    crate::ProjectContextState::NotCollected,
                    |publication| {
                        crate::ProjectContextState::Loaded(Arc::new(publication.snapshot.clone()))
                    },
                ),
            }),
            project_publications: Arc::new(restored.publications),
            context: restored.state.context,
            id: restored.state.session,
            notice: restored.notice,
            settings: SessionSettings {
                context_limits: settings.context_limits,
                slug: settings.slug,
                display_name: settings.display_name,
                effort: settings.effort,
                image_input: settings.image_input,
                instructions: Arc::new(settings.instructions),
                working_directory: settings.working_directory,
            },
            turn: restored.state.turn,
            previous_turns: restored.state.previous_turns,
            committed_items: restored.state.committed_items,
            execution_blocked: restored.state.execution_blocked,
            tool_catalog: settings
                .tools
                .into_iter()
                .map(|tool| (tool.name.clone(), tool.catalog_description()))
                .collect(),
            queue: restored.state.queue,
            queue_paused: restored.state.queue_paused,
        };
        self.next_turn = restored.next_turn;
        self.session = Some(ChatSession {
            memory: settings.memory.map(Arc::new),
            view: view.clone(),
            history: Some(restored.history),
            tools: Arc::new(tools),
            pending_results: BTreeMap::new(),
        });
        view
    }
}
