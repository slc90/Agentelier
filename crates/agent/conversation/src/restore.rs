use crate::conversation::ChatSession;
use crate::{
    Conversation, ConversationFailure, InitialSession, RecordEvent, SessionRecord, SessionSettings,
    SessionView, StoredImage, StoredState, TurnStatus,
};
use agentelier_llm_model::{
    ArchivedPayload, HistoryAppend, ModelFailure, ModelFailureKind, ProtocolArchive,
    ProtocolHistory,
};
use agentelier_tools::{
    ImagePayload, MAX_SESSION_IMAGE_BYTES, ToolCatalog, ToolContract, ToolStatus,
};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

pub struct RestoredSession {
    pub state: StoredState,
    initial: InitialSession,
    history: ProtocolHistory,
    tasks: crate::tasks::TaskLedger,
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
    results: BTreeMap<String, SavedResult>,
    started: HashSet<String>,
    appended: HashSet<String>,
    sequence: u64,
    turn: crate::TurnId,
    request_index: u32,
    in_history: bool,
    over_budget: bool,
    rejected: Vec<String>,
}

struct SavedResult {
    sequence: u64,
    output: ArchivedPayload,
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
    let mut seen_calls = HashSet::new();
    let mut tasks = crate::tasks::TaskLedger::default();
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
                    || fact.projection.tool_contract != initial.settings.tool_contract
                    || fact.projection.memory.as_ref().is_some_and(|material| {
                        Some(&material.snapshot) != initial.settings.memory.as_ref()
                    })
                    || fact.request != turn.request_index
                    || fact.window != incoming.context.window
                    || fact.publication != publications.last().map(|publication| publication.id)
                    || fact.projection.history.as_ref().is_some_and(|frozen| {
                        frozen.position != history.len()
                            || !history
                                .result_ids()
                                .eq(frozen.result_calls.iter().map(String::as_str))
                    })
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
                state: mut incoming,
                output,
                registrations,
                acceptance,
            } => {
                if pending.is_some() {
                    return Err(ConversationFailure::new("存档完整响应没有封闭边界"));
                }
                if !initial.settings.tool_contract.is_flat() {
                    output
                        .validate_tool_contract(
                            initial.settings.tool_contract,
                            &initial.settings.tools,
                        )
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    if output
                        .call_ids()
                        .any(|id| !seen_calls.insert(id.to_owned()))
                    {
                        return Err(ConversationFailure::new("存档重复使用已有 call_id"));
                    }
                }
                let turn = incoming
                    .turn
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("完整响应缺少任务身份"))?;
                let over_budget = matches!(&registrations, Some(crate::ResponseTasks::OverBudget));
                let mut rejected = Vec::new();
                if let Some(registrations) = registrations {
                    let calls = output
                        .function_calls()
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    let settled = if let Some(acceptance) = &acceptance {
                        let frozen = state
                            .turn
                            .as_ref()
                            .filter(|source| {
                                source.id == turn.id && source.request_index == turn.request_index
                            })
                            .and_then(|source| source.requests.last())
                            .and_then(|request| request.projection.history.as_ref())
                            .ok_or_else(|| ConversationFailure::new("存档响应缺少对应冻结请求"))?;
                        let expected =
                            tasks.plan_delivery(&calls, &frozen.result_calls, !over_budget);
                        if acceptance.delivery != expected {
                            return Err(ConversationFailure::new("存档响应的任务结算或准入不一致"));
                        }
                        rejected = expected.rejected;
                        expected.settled
                    } else {
                        let included: Vec<_> = history.result_ids().map(str::to_owned).collect();
                        tasks.plan_delivery(&[], &included, true).settled
                    };
                    registrations.validate(&calls, turn.id, turn.request_index, &rejected)?;
                    tasks.install(registrations, &settled, record.sequence, false)?;
                } else if acceptance.is_some() {
                    return Err(ConversationFailure::new("已接纳响应缺少任务登记"));
                }
                if let Some(acceptance) = &acceptance {
                    history
                        .append_response(
                            (!retained_input).then_some(turn.input.as_str()),
                            &output,
                            acceptance.append,
                        )
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    retained_input = true;
                    covered_sequence = record.sequence;
                    turn.committed_steps = turn
                        .committed_steps
                        .checked_add(1)
                        .ok_or_else(|| ConversationFailure::new("提交步骤数超出范围"))?;
                    for tool in &mut turn.tools {
                        if tool.request_index == turn.request_index {
                            tool.retained = true;
                        }
                    }
                    if output.has_compaction() {
                        incoming.context.anchor = None;
                    }
                }
                pending = Some(PendingResponse {
                    output,
                    results: BTreeMap::new(),
                    started: HashSet::new(),
                    appended: HashSet::new(),
                    sequence: record.sequence,
                    turn: turn.id,
                    request_index: turn.request_index,
                    in_history: acceptance.is_some(),
                    over_budget,
                    rejected,
                });
                merge(&mut state, incoming);
            }
            RecordEvent::ToolStarted {
                state: incoming,
                call_id,
                execution,
            } => {
                let response = pending
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("工具开始缺少完整响应"))?;
                if response.over_budget
                    || response.rejected.contains(&call_id)
                    || !response.output.call_ids().any(|id| id == call_id)
                    || !response.started.insert(call_id.clone())
                {
                    return Err(ConversationFailure::new("工具开始身份未知或重复"));
                }
                let turn = incoming
                    .turn
                    .as_ref()
                    .ok_or_else(|| ConversationFailure::new("工具开始缺少来源任务"))?;
                if turn.id != response.turn || turn.request_index != response.request_index {
                    return Err(ConversationFailure::new("工具开始不属于来源请求"));
                }
                tasks.transition(
                    turn.id,
                    turn.request_index,
                    &call_id,
                    execution,
                    ToolStatus::Running,
                    record.sequence,
                    true,
                )?;
                merge(&mut state, incoming);
            }
            RecordEvent::ToolReturned {
                state: incoming,
                call_id,
                status,
                output,
                execution,
            } => {
                let response = pending
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("工具结果缺少完整响应"))?;
                if ((response.over_budget || response.rejected.contains(&call_id))
                    && status != ToolStatus::NotExecuted)
                    || !response.output.call_ids().any(|id| id == call_id)
                    || (status != ToolStatus::NotExecuted && !response.started.contains(&call_id))
                    || response
                        .results
                        .insert(
                            call_id.clone(),
                            SavedResult {
                                sequence: record.sequence,
                                output,
                            },
                        )
                        .is_some()
                {
                    return Err(ConversationFailure::new(
                        "工具结果身份未知、重复或缺少开始事实",
                    ));
                }
                let turn = incoming
                    .turn
                    .as_ref()
                    .ok_or_else(|| ConversationFailure::new("工具结果缺少来源任务"))?;
                if turn.id != response.turn || turn.request_index != response.request_index {
                    return Err(ConversationFailure::new("工具结果不属于来源请求"));
                }
                tasks.transition(
                    turn.id,
                    turn.request_index,
                    &call_id,
                    execution,
                    status,
                    record.sequence,
                    true,
                )?;
                merge(&mut state, incoming);
            }
            RecordEvent::ResultAppended {
                state: incoming,
                call_id,
                saved_sequence,
                append,
            } => {
                let response = pending
                    .as_mut()
                    .filter(|response| response.in_history)
                    .ok_or_else(|| ConversationFailure::new("追加工具结果缺少已接纳的原始响应"))?;
                if !incoming.turn.as_ref().is_some_and(|turn| {
                    turn.id == response.turn && turn.request_index == response.request_index
                }) || !response.appended.insert(call_id.clone())
                {
                    return Err(ConversationFailure::new("工具结果追加身份重复或过期"));
                }
                let saved = response
                    .results
                    .get(&call_id)
                    .filter(|saved| saved.sequence == saved_sequence)
                    .ok_or_else(|| ConversationFailure::new("追加工具结果缺少已保存载荷"))?;
                tasks.append_result(&call_id, saved_sequence)?;
                history
                    .append_result(call_id, saved.output.clone(), append)
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                covered_sequence = record.sequence;
                merge(&mut state, incoming);
            }
            RecordEvent::ResponseClosed {
                state: incoming,
                response_sequence,
            } => {
                let response = pending
                    .take()
                    .filter(|response| {
                        response.in_history
                            && response.sequence == response_sequence
                            && response.output.call_ids().count() == response.appended.len()
                    })
                    .ok_or_else(|| ConversationFailure::new("批次结束前工具结果尚未全部追加"))?;
                if !incoming.turn.as_ref().is_some_and(|turn| {
                    turn.id == response.turn && turn.request_index == response.request_index
                }) {
                    return Err(ConversationFailure::new("批次结束不属于已接纳的请求"));
                }
                covered_sequence = record.sequence;
                merge(&mut state, incoming);
            }
            RecordEvent::Sealed {
                state: incoming,
                history: complete,
            } => {
                let response = pending
                    .take()
                    .ok_or_else(|| ConversationFailure::new("封闭步骤缺少完整响应"))?;
                if response.in_history
                    || response.output.call_ids().count() != response.results.len()
                {
                    return Err(ConversationFailure::new("封闭步骤缺少实际工具结果"));
                }
                tasks.restore_appended(complete.result_ids());
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
                if pending.as_ref().is_some_and(|response| response.in_history) {
                    return Err(ConversationFailure::new("终态不能替换尚未收齐结果的历史"));
                }
                history = finalized;
                state.queue_paused |= incoming.queue_paused;
                merge(&mut state, incoming);
            }
            RecordEvent::Finished {
                state: mut incoming,
                history: finished,
            } => {
                if let Some(turn) = &incoming.turn {
                    tasks.settle_turn(turn.id, record.sequence);
                }
                if let Some(finished) = finished {
                    if pending.as_ref().is_some_and(|response| response.in_history) {
                        return Err(ConversationFailure::new("已结束任务缺少已确认的批次终态"));
                    }
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
                if let Some(turn) = &incoming.turn {
                    tasks.settle_turn(turn.id, record.sequence);
                }
                state = incoming;
                covered_sequence = record.sequence;
                history = recovered;
                tasks.restore_appended(history.result_ids());
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
    if let Some(mut response) = pending {
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
                if let Some(saved) = response.results.get(call_id) {
                    tool.output = Some(match &saved.output {
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
            if response.in_history {
                for call_id in calls.iter().filter(|id| !response.appended.contains(*id)) {
                    let payload = response.results.remove(call_id).map_or_else(
                        || ArchivedPayload::interrupted(response.started.contains(call_id)),
                        |saved| saved.output,
                    );
                    let start = history.len();
                    history
                        .append_result(
                            call_id.clone(),
                            payload,
                            HistoryAppend {
                                start,
                                end: start + 1,
                            },
                        )
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                }
            } else {
                history
                    .seal_interrupted(
                        response.output,
                        response
                            .results
                            .into_iter()
                            .map(|(id, stored)| (id, stored.output))
                            .collect(),
                        &response.started,
                        (!retained_input).then_some(turn.input.as_str()),
                    )
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                turn.committed_steps = turn
                    .committed_steps
                    .checked_add(1)
                    .ok_or_else(|| ConversationFailure::new("恢复步骤数超出范围"))?;
            }
        }
        turn.status =
            TurnStatus::Failed("上次运行中断；已保留实际记录，缺失结果仅作为中断说明续接".into());
        normalized = true;
        notices.push(
            if calls.is_empty() && response.in_history {
                "上次运行收尾中断，已保留完整确认的模型输出"
            } else if calls.is_empty() {
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
    if normalized {
        tasks.restore_appended(history.result_ids());
    }
    let mut images = HashMap::<String, Arc<ImagePayload>>::new();
    if !initial.settings.tool_contract.is_flat() {
        history
            .validate_tool_contract(initial.settings.tool_contract, &initial.settings.tools)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    }
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
    if history.has_open_calls() {
        return Err(ConversationFailure::new(
            "当前执行阶段要求恢复时封闭所有调用",
        ));
    }
    if history.image_bytes_with(std::iter::empty()) > MAX_SESSION_IMAGE_BYTES {
        return Err(ConversationFailure::new("恢复的工作上下文超过会话图片预算"));
    }
    state.committed_items = history.summary();
    state.context.refresh(
        &history,
        &initial.settings.instructions,
        &initial.settings.tools,
    )?;
    tasks.recover_interrupted();
    Ok(RestoredSession {
        state,
        initial,
        history,
        tasks,
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
            settings.tool_contract,
            self.extensions.iter().chain(tools),
        )
        .and_then(|available| available.rebind(&settings.tools));
        let tools = match tools {
            _ if settings.tool_contract == ToolContract::ExperimentalNamespace => {
                restored.state.execution_blocked = true;
                restored.add_recovery_notice(
                    "此会话使用已结束的 namespace 实验契约，历史可查看。请新建会话继续聊天。",
                );
                ToolCatalog::default()
            }
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
            active: None,
            tasks: restored.tasks,
        });
        view
    }
}
