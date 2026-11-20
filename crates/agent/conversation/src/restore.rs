use crate::archive::{RecoveredResult, RecoveryEvidence};
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
    pub processes: crate::ProcessLedger,
    publications: Vec<Arc<agentelier_llm_model::ProjectPublication>>,
    next_turn: u64,
    notice: Option<String>,
    recovered_results: Vec<RecoveredResult>,
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
            history: None,
            notice: self
                .notice
                .clone()
                .unwrap_or_else(|| "已恢复有效历史，待办保持暂停".into()),
            results: Some(self.recovered_results.clone()),
        }
    }

    pub fn confirm_recovery(&mut self, sequence: u64) {
        self.tasks.confirm_recovery(sequence);
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
    status: ToolStatus,
    output: ArchivedPayload,
}

#[derive(Default)]
struct PendingResponses(BTreeMap<u64, PendingResponse>);

impl PendingResponses {
    fn has_barrier(&self) -> bool {
        self.0.values().any(|response| {
            !response.in_history
                || response.output.function_calls().any(|call| {
                    call.map_or(true, |call| {
                        call.mode() == agentelier_tools::ToolMode::Synchronous
                            && !response.appended.contains(&call.call_id)
                    })
                })
        })
    }

    fn source(&mut self, state: &StoredState) -> Option<&mut PendingResponse> {
        let turn = state.turn.as_ref()?;
        self.0.values_mut().find(|response| {
            response.turn == turn.id && response.request_index == turn.request_index
        })
    }

    fn call_source(&mut self, state: &StoredState, call_id: &str) -> Option<&mut PendingResponse> {
        let turn = state.call_turn(call_id)?;
        self.0.values_mut().find(|response| {
            response.turn == turn.id
                && response.request_index <= turn.request_index
                && response.output.call_ids().any(|id| id == call_id)
        })
    }

    fn recover(
        &mut self,
        history: &mut ProtocolArchive,
        state: &mut StoredState,
        retained_input: bool,
    ) -> Result<Vec<RecoveredResult>, ConversationFailure> {
        let mut facts = Vec::new();
        let mut responses = std::mem::take(&mut self.0);
        // Older logs only had one accepted response at a time and retain their recovery order.
        if responses
            .values()
            .filter(|response| response.in_history)
            .count()
            > 1
        {
            let mut saved = responses
                .values()
                .filter(|response| response.in_history)
                .flat_map(|response| {
                    response
                        .results
                        .iter()
                        .filter(|(id, _)| !response.appended.contains(*id))
                        .map(|(id, saved)| {
                            (
                                saved.sequence,
                                response.sequence,
                                id.clone(),
                                saved.output.clone(),
                            )
                        })
                })
                .collect::<Vec<_>>();
            saved.sort_by_key(|(sequence, ..)| *sequence);
            for (sequence, response_sequence, call_id, output) in saved {
                let response = responses
                    .get_mut(&response_sequence)
                    .ok_or_else(|| ConversationFailure::new("已保存的工具结果缺少来源响应"))?;
                response.appended.insert(call_id.clone());
                facts.push(RecoveredResult {
                    response_sequence,
                    call_id: call_id.clone(),
                    evidence: RecoveryEvidence::Saved { sequence },
                });
                let start = history.len();
                history
                    .append_result(
                        call_id,
                        output,
                        HistoryAppend {
                            start,
                            end: start + 1,
                        },
                    )
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
            }
        }
        for response in responses.into_values() {
            recover_response(history, state, response, retained_input, &mut facts)?;
        }
        Ok(facts)
    }
}

fn recover_response(
    history: &mut ProtocolArchive,
    state: &mut StoredState,
    response: PendingResponse,
    retained_input: bool,
    facts: &mut Vec<RecoveredResult>,
) -> Result<(), ConversationFailure> {
    let mut calls: Vec<_> = response.output.call_ids().map(str::to_owned).collect();
    let turn = state
        .turn
        .iter_mut()
        .map(Box::as_mut)
        .chain(state.previous_turns.iter_mut())
        .find(|turn| turn.id == response.turn)
        .ok_or_else(|| ConversationFailure::new("中断响应缺少来源任务"))?;
    if calls.is_empty() {
        return Ok(());
    }
    if response.output.has_compaction() {
        state.context.anchor = None;
    }
    // Preserve durable completion order before filling calls that have no saved result.
    if response.in_history {
        calls.sort_by_key(|id| {
            response
                .results
                .get(id)
                .map_or(u64::MAX, |saved| saved.sequence)
        });
    }
    for call_id in calls {
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == response.request_index && tool.call_id == call_id)
            .ok_or_else(|| ConversationFailure::new("中断响应缺少对应工具展示身份"))?;
        let started = response.started.contains(&call_id);
        let (status, evidence, output) = match response.results.get(&call_id) {
            Some(saved) => (
                saved.status,
                RecoveryEvidence::Saved {
                    sequence: saved.sequence,
                },
                saved.output.clone(),
            ),
            None if started => (
                ToolStatus::Unconfirmed,
                RecoveryEvidence::Unconfirmed,
                ArchivedPayload::interrupted(true),
            ),
            None => (
                ToolStatus::NotExecuted,
                RecoveryEvidence::NotExecuted,
                ArchivedPayload::interrupted(false),
            ),
        };
        tool.status = status;
        tool.output = Some(match &output {
            ArchivedPayload::Text { text } => text.clone(),
            ArchivedPayload::Image { summary, .. } => summary.clone(),
        });
        tool.retained = true;
        if response.appended.contains(&call_id) {
            continue;
        }
        facts.push(RecoveredResult {
            response_sequence: response.sequence,
            call_id: call_id.clone(),
            evidence,
        });
        if response.in_history {
            let start = history.len();
            history
                .append_result(
                    call_id,
                    output,
                    HistoryAppend {
                        start,
                        end: start + 1,
                    },
                )
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        }
    }
    if !response.in_history {
        history
            .seal_interrupted(
                response.output,
                response
                    .results
                    .into_iter()
                    .map(|(id, saved)| (id, saved.output))
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
    Ok(())
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

fn is_read_only(tools: &[crate::StoredTool], name: &str) -> bool {
    use agentelier_tools::ToolSource;

    tools
        .iter()
        .find(|tool| tool.name == name)
        .is_some_and(|tool| match &tool.source {
            ToolSource::Builtin => name == "view_image",
            ToolSource::Extension { provider, tool } => {
                provider == "memory" && matches!(tool.as_str(), "search_memory" | "read_memory")
            }
            ToolSource::Mcp { .. } => false,
        })
}

fn remember_process(
    processes: &mut BTreeMap<String, crate::TurnId>,
    tool: &crate::ToolView,
    output: &ArchivedPayload,
    turn: crate::TurnId,
) {
    if !matches!(tool.name.as_str(), "exec_command" | "write_stdin") {
        return;
    }
    let ArchivedPayload::Text { text } = output else {
        return;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let data = &value["data"];
    let Some(id) = data["session_id"].as_str() else {
        return;
    };
    match data["state"].as_str() {
        Some("running" | "cleanup_unconfirmed") => {
            processes.entry(id.to_owned()).or_insert(turn);
        }
        Some("exited") if data["cleanup_confirmed"].as_bool() == Some(true) => {
            processes.remove(id);
        }
        _ => {}
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
    incoming.task_policy = std::mem::take(&mut state.task_policy);
    *state = incoming;
}

fn merge_tool(state: &mut StoredState, incoming: StoredState, call_id: &str) {
    let Some(source) = incoming.call_turn(call_id) else {
        return;
    };
    let Some(target) = state
        .turn
        .iter_mut()
        .map(Box::as_mut)
        .chain(state.previous_turns.iter_mut())
        .find(|turn| turn.id == source.id)
    else {
        return;
    };
    let matches = |tool: &crate::ToolView| tool.call_id == call_id;
    if let Some((saved, current)) = source
        .tools
        .iter()
        .find(|tool| matches(tool))
        .zip(target.tools.iter_mut().find(|tool| matches(tool)))
    {
        current.status = saved.status;
        current.output.clone_from(&saved.output);
        current.retained |= saved.retained;
    }
}

fn recover_external_effects(
    state: &mut StoredState,
    tasks: &mut crate::tasks::TaskLedger,
    recovered: &[RecoveredResult],
    catalog: &[crate::StoredTool],
) {
    for result in recovered
        .iter()
        .filter(|result| result.evidence == RecoveryEvidence::Unconfirmed)
    {
        let tool = state
            .turn
            .iter_mut()
            .map(Box::as_mut)
            .chain(state.previous_turns.iter_mut())
            .flat_map(|turn| &mut turn.tools)
            .find(|tool| tool.retained && tool.call_id == result.call_id);
        if let Some(tool) = tool
            && catalog.iter().any(|definition| {
                definition.name == tool.name
                    && matches!(definition.source, agentelier_tools::ToolSource::Mcp { .. })
            })
        {
            tool.status = ToolStatus::ExternalUnconfirmed;
            state
                .task_policy
                .external_unconfirmed
                .insert(result.call_id.clone());
            tasks.recover_external_unconfirmed(&result.call_id);
            state.queue_paused = true;
        }
    }
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
        task_policy: Default::default(),
    };
    let mut publications = Vec::<Arc<agentelier_llm_model::ProjectPublication>>::new();
    let mut history = ProtocolArchive::default();
    if let Some(memory) = &initial.settings.memory {
        memory
            .validate()
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
    }
    let mut pending = PendingResponses::default();
    let mut seen_calls = HashSet::new();
    let mut seen_items = HashSet::new();
    let mut completed_requests = HashSet::new();
    let mut continued = None;
    let mut tasks = crate::tasks::TaskLedger::default();
    let mut retained_input = false;
    let mut covered_sequence = 1;
    let mut next_turn = 0;
    let mut notices = Vec::new();
    let mut historical_processes = BTreeMap::new();
    let mut processes = crate::ProcessLedger::default();
    let mut finalized_without_join = false;
    for record in records {
        if record.session != initial.session || record.event.session() != initial.session {
            return Err(ConversationFailure::new("存档事件属于另一会话"));
        }
        if let Some(snapshot) = record.event.state() {
            next_turn = next_turn.max(maximum(snapshot));
        }
        let prepared = matches!(&record.event, RecordEvent::RequestPrepared { .. });
        match record.event {
            RecordEvent::Process { fact } => {
                if matches!(fact.change, agentelier_tools::ProcessChange::Started)
                    && !state
                        .turn
                        .iter()
                        .map(Box::as_ref)
                        .chain(&state.previous_turns)
                        .any(|turn| {
                            turn.id.0 == fact.origin.turn
                                && turn.tools.iter().any(|tool| {
                                    tool.retained
                                        && tool.call_id == fact.origin.call_id
                                        && tool.name == "exec_command"
                                })
                        })
                {
                    return Err(ConversationFailure::new("进程登记缺少已保存的创建调用"));
                }
                processes.apply(&fact, record.sequence)?;
            }
            RecordEvent::ProjectPublished {
                state: incoming,
                publication,
                history: published,
            } => {
                publication
                    .validate()
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                if pending.has_barrier()
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
                if (fact.projection.route == agentelier_llm_model::RecordedModelSource::Codex)
                    != initial.settings.codex.is_some()
                {
                    return Err(ConversationFailure::new(
                        "冻结请求的模型来源与会话设置不一致",
                    ));
                }
                fact.projection
                    .validate_request_materials()
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                let skip_once = fact.projection.compaction
                    == Some(agentelier_llm_model::RequestCompaction::SkipOnce);
                if skip_once
                    != continued.is_some_and(|(id, request): (crate::TurnId, u32)| {
                        id == turn.id && request.checked_add(1) == Some(fact.request)
                    })
                {
                    return Err(ConversationFailure::new(
                        "单次跳过压缩的请求缺少一致的继续记录",
                    ));
                }
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
                if pending.has_barrier()
                    || (prepared
                        && fact
                            .task_registration_cutoff
                            .is_some_and(|cutoff| cutoff != tasks.revision()))
                    || fact.projection.tool_contract != initial.settings.tool_contract
                    || fact.projection.memory.as_ref().is_some_and(|material| {
                        Some(&material.snapshot) != initial.settings.memory.as_ref()
                    })
                    || fact.request != turn.request_index
                    || fact.window != incoming.context.window
                    || fact.publication != publications.last().map(|publication| publication.id)
                    || fact.projection.history.as_ref().is_some_and(|frozen| {
                        (prepared && frozen.position != history.len())
                            || !history.matches_request_history(
                                frozen,
                                fact.projection.compaction.is_some(),
                            )
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
                let new = checkpoint.prefix_hash.is_some();
                if (!new
                    && (!pending.0.is_empty()
                        || checkpoint.project_publication
                            != publications.last().map(|publication| publication.id)
                        || checkpoint.source_sequence != covered_sequence
                        || checkpoint.covered_items != history.len()))
                    || pending.0.values().any(|response| !response.in_history)
                    || checkpoint.project_publication.is_some_and(|id| {
                        !publications.iter().any(|publication| {
                            publication.id == id && publication.window <= checkpoint.source_window
                        })
                    })
                    || checkpoint.source_window != state.context.window
                    || checkpoint.source_sequence > covered_sequence
                    || checkpoint.source_sequence >= record.sequence
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
                if let Some(hash) = &checkpoint.prefix_hash {
                    history = history
                        .replace_compacted_prefix(
                            checkpoint.covered_items,
                            hash,
                            &checkpoint.history,
                        )
                        .map_err(crate::result_budget::model_error)?;
                    state.context = incoming.context;
                } else {
                    history = checkpoint.history;
                    merge(&mut state, incoming);
                }
                retained_input = checkpoint.input_retained;
                covered_sequence = record.sequence;
            }
            RecordEvent::Accepted { state: incoming } => {
                notices.clear();
                if pending.has_barrier() {
                    return Err(ConversationFailure::new("存档在未封闭响应之后开始另一任务"));
                }
                let mut incoming = incoming;
                incoming.task_policy.external_unconfirmed =
                    std::mem::take(&mut state.task_policy.external_unconfirmed);
                retained_input = incoming
                    .turn
                    .as_ref()
                    .is_some_and(|turn| turn.result_continuation);
                state = incoming;
                continued = None;
                covered_sequence = record.sequence;
            }
            RecordEvent::ContinuedWithoutCompaction {
                state: incoming,
                source,
            } => {
                let (old, next) = state
                    .turn
                    .as_ref()
                    .zip(incoming.turn.as_ref())
                    .ok_or_else(|| ConversationFailure::new("压缩继续缺少原任务"))?;
                if old.compaction_continuation != Some(source)
                    || !matches!(old.status, TurnStatus::Failed(_))
                    || !matches!(next.status, TurnStatus::Running)
                    || next.compaction_continuation.is_some()
                    || old.id != next.id
                    || old.input_id != source.input
                    || next.input_id != source.input
                    || next.input != old.input
                    || next.request_index != source.request
                    || source.input_retained != retained_input
                    || source.request >= crate::MAX_TURN_REQUESTS
                    || !pending.0.is_empty()
                {
                    return Err(ConversationFailure::new("压缩继续与失败请求或原输入不一致"));
                }
                continued = Some((next.id, source.request));
                notices.clear();
                merge(&mut state, incoming);
                covered_sequence = record.sequence;
            }
            RecordEvent::Response {
                state: mut incoming,
                output,
                registrations,
                acceptance,
            } => {
                if pending.has_barrier() || pending.source(&incoming).is_some() {
                    return Err(ConversationFailure::new("存档完整响应没有封闭边界"));
                }
                if !initial.settings.tool_contract.is_flat() {
                    output
                        .validate_tool_contract(
                            initial.settings.tool_contract,
                            &initial.settings.tools,
                        )
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    for call in output.function_calls() {
                        let call =
                            call.map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                        if !seen_calls.insert(call.call_id) || !seen_items.insert(call.id) {
                            return Err(ConversationFailure::new("存档重复使用已有调用身份"));
                        }
                    }
                }
                let turn = incoming
                    .turn
                    .as_mut()
                    .ok_or_else(|| ConversationFailure::new("完整响应缺少任务身份"))?;
                if !completed_requests.insert((turn.id.0, turn.request_index)) {
                    return Err(ConversationFailure::new("同一请求出现多份完整响应"));
                }
                let over_budget = matches!(&registrations, Some(crate::ResponseTasks::OverBudget));
                let mut rejected = Vec::new();
                if let Some(registrations) = registrations {
                    let calls = output
                        .function_calls()
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                    let settled = if let Some(acceptance) = &acceptance {
                        let projection = &state
                            .turn
                            .as_ref()
                            .filter(|source| {
                                source.id == turn.id && source.request_index == turn.request_index
                            })
                            .and_then(|source| source.requests.last())
                            .ok_or_else(|| ConversationFailure::new("存档响应缺少对应冻结请求"))?;
                        let frozen = projection
                            .projection
                            .history
                            .as_ref()
                            .ok_or_else(|| ConversationFailure::new("存档响应缺少冻结历史"))?;
                        if projection.projection.compaction.is_some()
                            && output.has_compaction()
                            && (!frozen.pending_calls.is_empty()
                                || !history.accepts_compaction(&output)
                                || projection.projection.compaction
                                    == Some(agentelier_llm_model::RequestCompaction::SkipOnce))
                        {
                            return Err(ConversationFailure::new(
                                "存档接受了越过开放调用或已禁用的压缩",
                            ));
                        }
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
                    if let Some(request) = turn.requests.last_mut() {
                        request.response_accepted = Some(true);
                    }
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
                pending.0.insert(
                    record.sequence,
                    PendingResponse {
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
                    },
                );
                merge(&mut state, incoming);
            }
            RecordEvent::TaskPolicyChanged { change, .. } => {
                state.task_policy.apply(&change)?;
                if matches!(change, crate::TaskPolicyChange::Pause) {
                    state.queue_paused = true;
                }
            }
            RecordEvent::TaskCancellationRequested { targets, .. } => {
                tasks.request_cancel(&targets, record.sequence)?;
            }
            RecordEvent::ToolStarted {
                state: incoming,
                call_id,
                execution,
            } => {
                let response = pending
                    .call_source(&incoming, &call_id)
                    .ok_or_else(|| ConversationFailure::new("工具开始缺少完整响应"))?;
                if response.over_budget
                    || response.rejected.contains(&call_id)
                    || !response.output.call_ids().any(|id| id == call_id)
                    || !response.started.insert(call_id.clone())
                {
                    return Err(ConversationFailure::new("工具开始身份未知或重复"));
                }
                let turn = incoming
                    .call_turn(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具开始缺少来源任务"))?;
                tasks.transition(
                    turn.id,
                    response.request_index,
                    &call_id,
                    execution,
                    ToolStatus::Running,
                    record.sequence,
                    true,
                )?;
                merge_tool(&mut state, incoming, &call_id);
            }
            RecordEvent::ToolReturned {
                state: incoming,
                call_id,
                status,
                output,
                execution,
            } => {
                if !matches!(
                    status,
                    ToolStatus::Succeeded
                        | ToolStatus::Failed
                        | ToolStatus::TimedOut
                        | ToolStatus::Cancelled
                        | ToolStatus::NotExecuted
                        | ToolStatus::Unconfirmed
                        | ToolStatus::ExternalUnconfirmed
                ) {
                    return Err(ConversationFailure::new("已保存的工具结果缺少终态"));
                }
                if let ArchivedPayload::Image { summary, image } = &output {
                    let loaded = load(&StoredImage {
                        session: initial.session,
                        summary: summary.clone(),
                        image: image.clone(),
                    })?;
                    if loaded.descriptor() != *image {
                        return Err(ConversationFailure::new("已保存工具结果的附件不匹配"));
                    }
                }
                let response = pending
                    .call_source(&incoming, &call_id)
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
                                status,
                                output: output.clone(),
                            },
                        )
                        .is_some()
                {
                    return Err(ConversationFailure::new(
                        "工具结果身份未知、重复或缺少开始事实",
                    ));
                }
                let turn = incoming
                    .call_turn(&call_id)
                    .ok_or_else(|| ConversationFailure::new("工具结果缺少来源任务"))?;
                let tool = turn
                    .tools
                    .iter()
                    .find(|tool| {
                        tool.call_id == call_id && tool.request_index == response.request_index
                    })
                    .ok_or_else(|| ConversationFailure::new("工具结果缺少展示身份"))?;
                if matches!(
                    initial
                        .settings
                        .tools
                        .iter()
                        .find(|definition| definition.name == tool.name)
                        .map(|definition| &definition.source),
                    Some(agentelier_tools::ToolSource::Builtin)
                ) {
                    remember_process(&mut historical_processes, tool, &output, turn.id);
                }
                tasks.transition(
                    turn.id,
                    response.request_index,
                    &call_id,
                    execution,
                    status,
                    record.sequence,
                    true,
                )?;
                merge_tool(&mut state, incoming, &call_id);
                if status == ToolStatus::ExternalUnconfirmed {
                    state.task_policy.external_unconfirmed.insert(call_id);
                    state.queue_paused = true;
                }
            }
            RecordEvent::ResultAppended {
                state: incoming,
                call_id,
                saved_sequence,
                append,
            } => {
                let response = pending
                    .call_source(&incoming, &call_id)
                    .filter(|response| response.in_history)
                    .ok_or_else(|| ConversationFailure::new("追加工具结果缺少已接纳的原始响应"))?;
                if !response.appended.insert(call_id.clone()) {
                    return Err(ConversationFailure::new("工具结果追加身份重复或过期"));
                }
                let saved = response
                    .results
                    .get(&call_id)
                    .filter(|saved| saved.sequence == saved_sequence)
                    .ok_or_else(|| ConversationFailure::new("追加工具结果缺少已保存载荷"))?;
                tasks.append_result(&call_id, saved_sequence, record.sequence)?;
                history
                    .append_result(call_id.clone(), saved.output.clone(), append)
                    .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
                covered_sequence = record.sequence;
                merge_tool(&mut state, incoming, &call_id);
            }
            RecordEvent::ResponseClosed {
                state: incoming,
                response_sequence,
            } => {
                let response = pending
                    .0
                    .remove(&response_sequence)
                    .filter(|response| {
                        response.in_history
                            && response.sequence == response_sequence
                            && response.output.call_ids().count() == response.appended.len()
                    })
                    .ok_or_else(|| ConversationFailure::new("批次结束前工具结果尚未全部追加"))?;
                if !incoming
                    .turn
                    .iter()
                    .map(Box::as_ref)
                    .chain(&incoming.previous_turns)
                    .any(|turn| {
                        turn.id == response.turn && turn.request_index >= response.request_index
                    })
                {
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
                    .0
                    .pop_first()
                    .map(|(_, response)| response)
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
                if pending.has_barrier() {
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
                    || state.execution_blocked
                    || !state.task_policy.external_unconfirmed.is_empty()
                    || (state.queue_paused && !matches!(decision, crate::QueueDecision::Resumed));
                if matches!(decision, crate::QueueDecision::Resumed) && !state.queue_paused {
                    state.task_policy.background_paused = false;
                }
            }
            RecordEvent::Finalized {
                state: incoming,
                history: finalized,
            } => {
                finalized_without_join = true;
                if pending.has_barrier() {
                    return Err(ConversationFailure::new("前台终态仍有未确认响应或同步调用"));
                }
                history = finalized;
                state.queue_paused |= incoming.queue_paused;
                merge(&mut state, incoming);
            }
            RecordEvent::Finished {
                state: mut incoming,
                history: finished,
            } => {
                finalized_without_join = false;
                if let Some(finished) = finished.filter(|_| !pending.has_barrier()) {
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
                results,
            } => {
                finalized_without_join = false;
                processes.historical();
                let recovered = match (recovered, results) {
                    (None, Some(results)) => {
                        let expected = pending.recover(&mut history, &mut state, retained_input)?;
                        if expected != results {
                            return Err(ConversationFailure::new("恢复结果与已确认工具事实不一致"));
                        }
                        expected
                    }
                    (Some(recovered), None) => {
                        let facts = pending.recover(&mut history, &mut state, retained_input)?;
                        history = recovered;
                        facts
                    }
                    _ => return Err(ConversationFailure::new("恢复记录的历史材料无效")),
                };
                let mut incoming = incoming;
                incoming.task_policy = state.task_policy;
                state = incoming;
                covered_sequence = record.sequence;
                tasks.recover_interrupted();
                recover_external_effects(
                    &mut state,
                    &mut tasks,
                    &recovered,
                    &initial.settings.tools,
                );
                tasks.restore_appended(history.result_ids());
                tasks.confirm_recovery(record.sequence);
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
    let mut normalized = !pending.0.is_empty() || processes.has_active() || finalized_without_join;
    let recovered_results = pending.recover(&mut history, &mut state, retained_input)?;
    recover_external_effects(
        &mut state,
        &mut tasks,
        &recovered_results,
        &initial.settings.tools,
    );
    let missing_results = recovered_results
        .iter()
        .any(|result| !matches!(result.evidence, RecoveryEvidence::Saved { .. }));
    if missing_results {
        notices.push("已恢复确认的模型输出，缺失的工具结果已按开始事实补充说明".into());
    }
    let mut saved_material = None;
    if let Some(turn) = &mut state.turn {
        if matches!(turn.status, TurnStatus::Running) {
            normalized = true;
            let response_saved = completed_requests.contains(&(turn.id.0, turn.request_index));
            let interruption = if response_saved {
                if !missing_results {
                    let saved_tools = turn.tools.iter().any(|tool| {
                        tool.request_index == turn.request_index
                            && tool.retained
                            && tool.output.is_some()
                    });
                    saved_material = Some(if saved_tools {
                        "工具结果已保存"
                    } else {
                        "已保留完整响应"
                    });
                }
                "上次运行在完整响应接纳后中断"
            } else {
                if turn.requests.last().is_some_and(|request| {
                    request.request == turn.request_index && request.sending_started
                }) {
                    notices.push("上次请求已开始发送，服务结果未知；已保存的工具结果保留".into());
                } else {
                    notices.push("未完成的模型输出没有进入工作上下文".into());
                }
                "上次运行在响应完成前中断"
            };
            turn.status = TurnStatus::Failed(interruption.into());
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
    for process in processes.views() {
        historical_processes.remove(&process.session_id);
    }
    for (id, origin) in historical_processes {
        let mut turns = state
            .turn
            .iter_mut()
            .map(Box::as_mut)
            .chain(&mut state.previous_turns);
        if let Some(turn) = turns.find(|turn| turn.id == origin)
            && !turn
                .process_cleanup
                .iter()
                .any(|process| process.session_id == id)
        {
            turn.process_cleanup.push(agentelier_tools::ProcessCleanup {
                session_id: id,
                cleanup_confirmed: false,
                output: "历史进程的结束与回收没有确认记录".into(),
            });
            normalized = true;
        }
    }
    state.execution_blocked = state
        .turn
        .iter()
        .map(Box::as_ref)
        .chain(&state.previous_turns)
        .any(|turn| {
            turn.process_cleanup
                .iter()
                .any(|process| !process.cleanup_confirmed)
                || turn.tools.iter().any(|tool| {
                    tool.retained
                        && tool.status == ToolStatus::Unconfirmed
                        && !is_read_only(&initial.settings.tools, &tool.name)
                })
        });
    if let Some(material) = saved_material {
        notices.push(if state.execution_blocked {
            material.into()
        } else {
            format!("{material}，等待用户继续")
        });
    }
    if state.execution_blocked {
        state.queue_paused = true;
        let notice = "上次工具的外部效果或清理未确认，当前会话已暂停执行";
        if !notices.iter().any(|saved| saved.contains(notice)) {
            notices.push(notice.into());
        }
    }
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
    processes.historical();
    Ok(RestoredSession {
        state,
        initial,
        history,
        tasks,
        processes,
        publications,
        next_turn,
        notice: (!notices.is_empty()).then(|| notices.join("\n")),
        normalized,
        recovered_results,
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
        let tools = if settings.codex.is_none() {
            restored.state.execution_blocked = true;
            restored.add_recovery_notice(
                "此会话的 SIWC 授权已下线。历史可查看，请新建 Codex 会话继续聊天。",
            );
            ToolCatalog::default()
        } else {
            let tools = ToolCatalog::with_extensions(
                &settings.working_directory,
                settings.image_input,
                settings.tool_contract,
                self.extensions.iter().chain(tools),
            )
            .and_then(|available| available.rebind(&settings.tools));
            match tools {
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
            settings: Box::new(SessionSettings {
                codex: settings.codex,
                context_limits: settings.context_limits,
                slug: settings.slug,
                display_name: settings.display_name,
                effort: settings.effort,
                image_input: settings.image_input,
                instructions: Arc::new(settings.instructions),
                working_directory: settings.working_directory,
            }),
            turn: restored.state.turn,
            previous_turns: restored.state.previous_turns,
            committed_items: restored.state.committed_items,
            execution_blocked: restored.state.execution_blocked,
            process_cleanup_unconfirmed: false,
            tool_catalog: settings
                .tools
                .into_iter()
                .map(|tool| (tool.name.clone(), tool.catalog_description()))
                .collect(),
            queue: restored.state.queue,
            queue_paused: restored.state.queue_paused,
            task_policy: Box::new(restored.state.task_policy),
        };
        self.next_turn = restored.next_turn;
        self.session = Some(ChatSession {
            idle: None,
            processes: restored.processes,
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
