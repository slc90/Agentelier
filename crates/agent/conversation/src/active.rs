use crate::control::{MAX_PENDING_STEERS, SteerInput, SteerState, SteerView};
use crate::conversation::{
    ConversationFailure, SessionView, TaskPhase, ToolView, TurnStatus, TurnView,
};
use agentelier_llm_model::{
    FunctionCall, GenerationRequest, GenerationResult, HistoryAppend, OutputText, ProtocolHistory,
    StreamEvent,
};
use agentelier_tools::{
    MAX_SESSION_IMAGE_BYTES, MAX_TOOL_BODY_BYTES, ToolCatalog, ToolPayload, ToolResult, ToolStatus,
};
use std::sync::Arc;

pub(crate) struct ActiveState {
    pub(crate) memory: Option<Arc<agentelier_llm_model::MemorySnapshot>>,
    pub(crate) source_sequence: Option<u64>,
    pub(crate) response_sequence: Option<u64>,
    pub(crate) history_confirmed: bool,
    pub(crate) input_retained: bool,
    pub(crate) accepting_steers: bool,
    pub(crate) completion: Option<TurnStatus>,
}

pub(crate) struct ToolResultEntry {
    output: ToolPayload,
    saved: Option<u64>,
}

pub struct ActiveTurn<'a> {
    pub(crate) state: &'a mut ActiveState,
    pub(crate) view: &'a mut SessionView,
    pub(crate) history: &'a mut ProtocolHistory,
    pub(crate) tools: &'a Arc<ToolCatalog>,
    pub(crate) results: &'a mut std::collections::BTreeMap<String, ToolResultEntry>,
    pub(crate) tasks: &'a mut crate::tasks::TaskLedger,
}

impl ActiveTurn<'_> {
    pub fn skills(
        &mut self,
        catalog: agentelier_skills::Catalog,
        selected: Vec<agentelier_skills::SelectedSkill>,
    ) -> Result<(), ConversationFailure> {
        let rendered = catalog
            .render()
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.view.context.task_material_tokens =
            agentelier_llm_model::estimated_text_tokens(&rendered)
                .saturating_add(agentelier_llm_model::estimated_text_tokens("developer"))
                .saturating_add(
                    self.state
                        .memory
                        .as_ref()
                        .map_or(0, |memory| memory.estimated_tokens()),
                );
        for skill in &selected {
            skill
                .validate()
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
            self.view.context.task_material_tokens = self
                .view
                .context
                .task_material_tokens
                .saturating_add(agentelier_llm_model::estimated_text_tokens(&skill.render()))
                .saturating_add(agentelier_llm_model::estimated_text_tokens("user"));
        }
        self.view
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("选中 Skills 缺少任务"))?
            .selected_skills = selected;
        self.view
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("Skills 目录缺少活动任务"))?
            .skills = Some(Box::new(catalog));
        Ok(())
    }
    pub fn model_context(
        &mut self,
        model: agentelier_llm_model::ModelInfo,
    ) -> Result<(), ConversationFailure> {
        if model.slug != self.view.settings.slug {
            return Err(ConversationFailure::new("上下文能力不属于会话模型"));
        }
        if self.view.context.limits.comp_hash != model.context_limits.comp_hash {
            self.view.context.anchor = None;
        }
        self.view.context.limits = model.context_limits;
        if !self.view.context.limits.available() {
            return Err(ConversationFailure::new(
                "当前模型目录未提供有效的服务端压缩阈值，无法启动请求",
            ));
        }
        if let Some(effort) = &self.view.settings.effort
            && !model.supported_reasoning_levels.is_empty()
            && !model.supported_reasoning_levels.contains(effort)
        {
            return Err(ConversationFailure::new(
                "当前账号模型不支持会话固定的推理设置",
            ));
        }
        Ok(())
    }
    pub fn stored_state(&self) -> crate::StoredState {
        let mut state = crate::StoredState::from(&*self.view);
        if let Some(status) = &self.state.completion
            && let Some(turn) = &mut state.turn
        {
            turn.status = status.clone();
        }
        state
    }

    pub fn archive(&self) -> agentelier_llm_model::ProtocolArchive {
        self.history.archive()
    }
    pub fn reaping(&mut self) -> Result<(), ConversationFailure> {
        self.view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?
            .phase = TaskPhase::ReapingProcesses;
        Ok(())
    }

    pub fn process_cleanup(
        &mut self,
        report: agentelier_tools::CleanupReport,
    ) -> Result<(), ConversationFailure> {
        self.view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?
            .process_cleanup
            .extend(report.processes);
        if report.failed {
            self.block_execution();
        }
        Ok(())
    }

    pub fn block_execution(&mut self) {
        self.view.execution_blocked = true;
    }

    pub fn use_memory_epoch(&mut self, epoch: Option<i64>) {
        if self
            .state
            .memory
            .as_ref()
            .is_some_and(|memory| Some(memory.epoch) != epoch)
            && let Some(memory) = self.state.memory.take()
        {
            self.view.context.task_material_tokens = self
                .view
                .context
                .task_material_tokens
                .saturating_sub(memory.estimated_tokens());
        }
    }

    pub fn tool_user_turn(
        &self,
        memory_epoch: Option<i64>,
    ) -> Result<agentelier_tools::ToolUserTurn, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("工具调用缺少当前用户任务"))?;
        let statements = std::iter::once(turn.input.clone())
            .chain(
                turn.steers
                    .iter()
                    .filter(|steer| matches!(steer.state, SteerState::Applied))
                    .map(|steer| steer.text.clone()),
            )
            .collect();
        Ok(agentelier_tools::ToolUserTurn {
            memory_epoch,
            session: self.view.id,
            turn: turn.id.0,
            input: turn.input_id.0,
            statements,
            selected_skills: turn
                .selected_skills
                .iter()
                .map(|selected| (selected.skill.path.clone(), selected.hash.clone()))
                .collect(),
        })
    }

    pub fn settings(&self) -> crate::SessionSettings {
        self.view.settings.clone()
    }

    pub fn session_id(&self) -> crate::SessionId {
        self.view.id
    }

    pub fn validate_record(&self, event: &crate::RecordEvent) -> Result<(), ConversationFailure> {
        let state = event.state();
        let (stored, live) = state
            .turn
            .as_ref()
            .zip(self.view.turn.as_ref())
            .ok_or_else(|| ConversationFailure::new("会话记录缺少任务身份"))?;
        if state.session != self.view.id
            || stored.id != live.id
            || stored.operation_id != live.operation_id
            || stored.request_index != live.request_index
        {
            return Err(ConversationFailure::new("会话记录不属于当前任务"));
        }
        let (call_id, execution, status) = match event {
            crate::RecordEvent::Response {
                acceptance: Some(acceptance),
                output,
                ..
            } => {
                let append = acceptance.append;
                if self.state.response_sequence.is_some()
                    || append.start != self.history.position()
                    || append.end.checked_sub(append.start)
                        != Some(output.len() + usize::from(!self.state.input_retained))
                {
                    return Err(ConversationFailure::new("会话响应追加位置已过期"));
                }
                return Ok(());
            }
            crate::RecordEvent::ResponseClosed {
                response_sequence, ..
            } => {
                if self.state.response_sequence != Some(*response_sequence) || self.has_open_calls()
                {
                    return Err(ConversationFailure::new("批次仍有未确认的工具结果"));
                }
                return Ok(());
            }
            crate::RecordEvent::ResultAppended {
                call_id,
                saved_sequence,
                append,
                ..
            } => {
                if self.result_append(call_id, *saved_sequence)? != *append {
                    return Err(ConversationFailure::new("会话工具结果追加位置已过期"));
                }
                return Ok(());
            }
            crate::RecordEvent::ToolStarted {
                call_id, execution, ..
            } => (call_id, *execution, ToolStatus::Running),
            crate::RecordEvent::ToolReturned {
                call_id,
                execution,
                status,
                ..
            } => (call_id, *execution, *status),
            _ => return Ok(()),
        };
        self.tasks.matching_transition(
            live.id,
            live.request_index,
            call_id,
            execution,
            status,
            false,
        )?;
        if !live.tools.iter().any(|tool| {
            tool.request_index == live.request_index
                && &tool.call_id == call_id
                && tool.status == status
        }) {
            return Err(ConversationFailure::new("工具记录不属于当前响应的调用状态"));
        }
        Ok(())
    }

    pub fn input(&self) -> Result<String, ConversationFailure> {
        self.view
            .turn
            .as_ref()
            .map(|turn| turn.input.clone())
            .ok_or_else(|| ConversationFailure::new("活动任务缺少用户输入"))
    }

    pub fn context_window(&self) -> u64 {
        self.view.context.window
    }

    pub fn request(&mut self) -> Result<GenerationRequest, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        turn.request_index = turn
            .request_index
            .checked_add(1)
            .ok_or(ConversationFailure::new("模型请求序号已耗尽"))?;
        turn.phase = TaskPhase::RequestingModel;
        let context = if !self.state.input_retained {
            self.history.with_user(&turn.input)
        } else {
            self.history.clone()
        };
        turn.request_items = context.summary();
        self.view.context.refresh(
            &context,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        Ok(GenerationRequest {
            history_position: self.history.position(),
            memory: self.state.memory.as_deref().cloned(),
            selected_skills: turn.selected_skills.clone(),
            skill_catalog: turn
                .skills
                .as_ref()
                .map(|catalog| catalog.render())
                .transpose()
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?,
            slug: self.view.settings.slug.clone(),
            compact_threshold: self
                .view
                .context
                .limits
                .compact_threshold()
                .ok_or_else(|| ConversationFailure::new("模型缺少有效的服务端压缩阈值"))?,
            effort: self.view.settings.effort.clone(),
            context,
            tools: self.tools.clone(),
            instructions: self.view.settings.instructions.clone(),
        })
    }

    pub fn update(&mut self, event: StreamEvent) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        match event {
            StreamEvent::Sending | StreamEvent::ResponseReceived { .. } => {
                let fact = turn
                    .requests
                    .last_mut()
                    .filter(|fact| fact.request == turn.request_index)
                    .ok_or_else(|| ConversationFailure::new("发送进度缺少准备记录"))?;
                fact.sending_started = true;
                if let StreamEvent::ResponseReceived { status } = event {
                    fact.response_status = Some(status);
                }
            }
            StreamEvent::Compacting => turn.phase = TaskPhase::CompactingContext,
            StreamEvent::Usage { response_id, usage } => {
                self.view.context.latest = Some(crate::UsageFact {
                    window: self.view.context.window,
                    turn: turn.id.0,
                    request: turn.request_index,
                    response_id,
                    usage,
                });
            }

            StreamEvent::FunctionCall(call) => {
                Self::upsert_tool(turn, call, ToolStatus::Receiving, self.tools)?;
            }
            StreamEvent::Item(mut message) => {
                turn.phase = TaskPhase::RequestingModel;
                message.request_index = turn.request_index;
                if let Some(existing) = turn.messages.iter_mut().find(|existing| {
                    existing.output_index == message.output_index
                        && existing.request_index == turn.request_index
                }) {
                    if existing.id != message.id {
                        return Err(ConversationFailure::new("模型输出项身份不一致"));
                    }
                    *existing = message;
                } else {
                    turn.messages.push(message);
                }
                turn.messages
                    .sort_by_key(|message| (message.request_index, message.output_index));
            }
            StreamEvent::Part {
                item_id,
                output_index,
                part,
            } => {
                let message = turn
                    .messages
                    .iter_mut()
                    .find(|message| {
                        message.output_index == output_index
                            && message.id == item_id
                            && message.request_index == turn.request_index
                    })
                    .ok_or(ConversationFailure::new("正文片段缺少对应输出项"))?;
                if let Some(existing) = message
                    .content
                    .iter_mut()
                    .find(|existing| existing.content_index == part.content_index)
                {
                    *existing = part;
                } else {
                    message.content.push(part);
                }
                message.content.sort_by_key(|part| part.content_index);
            }
            StreamEvent::Delta {
                item_id,
                output_index,
                content_index,
                text,
            } => {
                let message = turn
                    .messages
                    .iter_mut()
                    .find(|message| {
                        message.output_index == output_index
                            && message.id == item_id
                            && message.request_index == turn.request_index
                    })
                    .ok_or(ConversationFailure::new("文本增量缺少对应输出项"))?;
                if let Some(part) = message
                    .content
                    .iter_mut()
                    .find(|part| part.content_index == content_index)
                {
                    part.text.push_str(&text);
                } else {
                    message.content.push(OutputText {
                        content_index,
                        text,
                    });
                    message.content.sort_by_key(|part| part.content_index);
                }
            }
        }
        Ok(())
    }

    pub fn accept_steer(&mut self, input: SteerInput) -> Result<(), ConversationFailure> {
        if !self.state.accepting_steers {
            return Err(ConversationFailure::new("任务已结束，补充输入未接纳"));
        }
        if !agentelier_skills::mentions(&input.text).is_empty() {
            return Err(ConversationFailure::new(
                "运行中不能通过补充输入选择 Skill，请排队或开始新任务",
            ));
        }
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        if input.session != self.view.id
            || input.turn != turn.id
            || input.operation_id != turn.operation_id
            || !matches!(turn.status, TurnStatus::Running)
        {
            return Err(ConversationFailure::new("补充输入不属于当前活动任务"));
        }
        if input.text.trim().is_empty() {
            return Err(ConversationFailure::new("不能提交空白补充输入"));
        }
        if turn.steers.iter().any(|previous| previous.id == input.id) {
            return Err(ConversationFailure::new("补充输入身份重复"));
        }
        if turn
            .steers
            .iter()
            .filter(|input| matches!(input.state, SteerState::Accepted))
            .count()
            >= MAX_PENDING_STEERS
        {
            return Err(ConversationFailure::new("待生效补充输入已满"));
        }
        turn.steers.push(SteerView {
            id: input.id,
            text: input.text,
            state: SteerState::Accepted,
            applied_request: None,
        });
        Ok(())
    }

    pub fn has_pending_steer(&self) -> bool {
        self.view.turn.as_ref().is_some_and(|turn| {
            turn.steers
                .iter()
                .any(|input| matches!(input.state, SteerState::Accepted))
        })
    }

    pub fn close_steers_if_idle(&mut self) -> bool {
        if self.has_pending_steer() {
            return false;
        }
        self.state.accepting_steers = false;
        true
    }

    pub fn apply_steers(&mut self) -> Result<Vec<u64>, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        let pending = turn
            .steers
            .iter()
            .any(|input| matches!(input.state, SteerState::Accepted));
        if !pending {
            return Ok(Vec::new());
        }
        let request_index = turn
            .request_index
            .checked_add(1)
            .ok_or(ConversationFailure::new("请求序号已耗尽"))?;
        if !self.state.input_retained {
            *self.history = self.history.with_user(&turn.input);
            self.state.input_retained = true;
        }
        let mut applied = Vec::new();
        for input in &mut turn.steers {
            if matches!(input.state, SteerState::Accepted) {
                *self.history = self.history.with_user(&input.text);
                input.state = SteerState::Applied;
                input.applied_request = Some(request_index);
                applied.push(input.id);
            }
        }
        self.view.committed_items = self.history.summary();
        Ok(applied)
    }

    pub fn catalog(&self) -> Arc<ToolCatalog> {
        self.tools.clone()
    }

    fn upsert_tool(
        turn: &mut TurnView,
        call: FunctionCall,
        status: ToolStatus,
        catalog: &ToolCatalog,
    ) -> Result<(), ConversationFailure> {
        let existing = turn.tools.iter().position(|tool| {
            tool.request_index == turn.request_index && tool.output_index == call.output_index
        });
        if let Some(index) = existing {
            let tool = &turn.tools[index];
            if tool.id != call.id || tool.call_id != call.call_id || tool.name != call.name {
                return Err(ConversationFailure::new("工具输出身份冲突"));
            }
        }
        let mut arguments = call.arguments;
        let arguments_truncated = arguments.len() > MAX_TOOL_BODY_BYTES;
        if arguments_truncated {
            let mut end = MAX_TOOL_BODY_BYTES;
            while !arguments.is_char_boundary(end) {
                end -= 1;
            }
            arguments.truncate(end);
        }
        let view = ToolView {
            request_index: turn.request_index,
            id: call.id,
            output_index: call.output_index,
            call_id: call.call_id,
            source: catalog.display_source(&call.name).map(str::to_owned),
            name: call.name,
            arguments,
            arguments_truncated,
            status,
            output: None,
            retained: false,
        };
        if let Some(index) = existing {
            turn.tools[index] = view;
        } else {
            turn.tools.push(view);
        }
        Ok(())
    }

    pub fn accept_response(
        &mut self,
        result: &GenerationResult,
        memory_epoch: Option<i64>,
        within_budget: bool,
    ) -> Result<(crate::ResponseTasks, crate::ResponseAcceptance), ConversationFailure> {
        let previous_calls: std::collections::HashSet<_> = self
            .view
            .previous_turns
            .iter()
            .flat_map(|turn| &turn.tools)
            .filter(|tool| tool.retained)
            .map(|tool| tool.call_id.as_str())
            .collect();
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        if result.calls.iter().any(|call| {
            previous_calls.contains(call.call_id.as_str())
                || turn.tools.iter().any(|tool| {
                    tool.request_index < turn.request_index && tool.call_id == call.call_id
                })
        }) {
            return Err(ConversationFailure::new("会话重复使用已有 call_id"));
        }
        turn.phase = if result.calls.is_empty() {
            TaskPhase::RequestingModel
        } else {
            TaskPhase::ExecutingTool
        };
        turn.messages
            .retain(|message| message.request_index != turn.request_index);
        turn.messages
            .extend(result.messages.iter().cloned().map(|mut message| {
                message.request_index = turn.request_index;
                message
            }));
        for call in &result.calls {
            Self::upsert_tool(turn, call.clone(), ToolStatus::Pending, self.tools)?;
        }
        let append = self
            .history
            .response_append(
                (!self.state.input_retained).then_some(turn.input.as_str()),
                &result.output,
            )
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        let history = turn
            .requests
            .last()
            .filter(|request| request.request == turn.request_index)
            .and_then(|request| request.projection.history.as_ref())
            .ok_or_else(|| ConversationFailure::new("完整响应缺少冻结请求的历史集合"))?;
        let (registrations, delivery) = self.tasks.prepare(
            &result.calls,
            turn.id,
            turn.request_index,
            memory_epoch,
            within_budget,
            &history.result_calls,
        )?;
        Ok((
            registrations,
            crate::ResponseAcceptance { append, delivery },
        ))
    }

    pub fn confirm_tasks(
        &mut self,
        registrations: crate::ResponseTasks,
        delivery: &[crate::TaskId],
        sequence: u64,
    ) -> Result<bool, ConversationFailure> {
        self.tasks.install(registrations, delivery, sequence, true)
    }

    pub fn task_execution(&self, call_id: &str) -> Option<crate::TaskExecution> {
        self.tasks.execution(call_id)
    }

    pub fn has_open_calls(&self) -> bool {
        self.history.has_open_calls()
    }

    pub fn confirm_task_status(
        &mut self,
        call_id: &str,
        execution: Option<crate::TaskExecution>,
        status: ToolStatus,
        sequence: u64,
    ) -> Result<bool, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("工具状态更新缺少活动轮次"))?;
        self.tasks.transition(
            turn.id,
            turn.request_index,
            call_id,
            execution,
            status,
            sequence,
            false,
        )
    }

    pub fn confirm_result_saved(
        &mut self,
        call_id: &str,
        execution: Option<crate::TaskExecution>,
        status: ToolStatus,
        sequence: u64,
    ) -> Result<bool, ConversationFailure> {
        let result = self
            .results
            .get_mut(call_id)
            .filter(|entry| entry.saved.is_none())
            .ok_or_else(|| ConversationFailure::new("保存回执缺少唯一工具载荷"))?;
        result.saved = Some(sequence);
        self.confirm_task_status(call_id, execution, status, sequence)
    }

    pub fn tool_started(&mut self, call_id: &str) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        turn.phase = TaskPhase::ExecutingTool;
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == turn.request_index && tool.call_id == call_id)
            .ok_or(ConversationFailure::new("执行工具缺少调用记录"))?;
        tool.status = ToolStatus::Running;
        Ok(())
    }

    pub fn tool_result(
        &mut self,
        call_id: &str,
        mut result: ToolResult,
    ) -> Result<Option<(ToolStatus, ToolPayload)>, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("活动任务缺少轮次"))?;
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == turn.request_index && tool.call_id == call_id)
            .ok_or_else(|| ConversationFailure::new("工具结果缺少调用记录"))?;
        let previous = self
            .results
            .get(call_id)
            .map(|entry| &entry.output)
            .or_else(|| self.history.result_payload(call_id));
        if let Some(previous) = previous {
            if tool.status == result.status
                && agentelier_llm_model::ArchivedPayload::capture(previous)
                    == agentelier_llm_model::ArchivedPayload::capture(&result.output)
            {
                return Ok(None);
            }
            return Err(ConversationFailure::new("同一调用收到互相冲突的工具结果"));
        }
        if result.output.image().is_some()
            && self.history.image_bytes_with(
                self.results
                    .values()
                    .map(|entry| &entry.output)
                    .chain(std::iter::once(&result.output)),
            ) > MAX_SESSION_IMAGE_BYTES
        {
            result = result.reject_image_budget();
        }
        tool.status = result.status;
        tool.output = Some(result.output.summary().to_owned());
        self.results.insert(
            call_id.to_owned(),
            ToolResultEntry {
                output: result.output.clone(),
                saved: None,
            },
        );
        Ok(Some((result.status, result.output)))
    }

    pub fn confirm_response_history(
        &mut self,
        result: GenerationResult,
        append: HistoryAppend,
        sequence: u64,
    ) -> Result<Option<agentelier_llm_model::CompactionMetadata>, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        let coverage = turn
            .request_items
            .len()
            .checked_add(result.output.len())
            .ok_or_else(|| ConversationFailure::new("上下文用量覆盖位置超出范围"))?;
        let usage = result.usage.map(|usage| crate::UsageFact {
            window: self.view.context.window,
            turn: turn.id.0,
            request: turn.request_index,
            response_id: result.response_id,
            usage,
        });
        if append.start != self.history.position() || append.end != coverage {
            return Err(ConversationFailure::new("响应历史位置与回执不一致"));
        }
        self.history
            .append_response(
                (!self.state.input_retained).then_some(turn.input.as_str()),
                result.output,
            )
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.state.input_retained = true;
        turn.committed_steps = turn
            .committed_steps
            .checked_add(1)
            .ok_or_else(|| ConversationFailure::new("提交步骤数超出范围"))?;
        for tool in &mut turn.tools {
            if tool.request_index == turn.request_index {
                tool.retained = true;
            }
        }
        self.view.committed_items = self.history.summary();
        let compacted = result.compaction.is_some();
        if compacted {
            self.view.context.anchor = None;
        }
        if let Some(fact) = usage {
            self.view.context.latest = Some(fact.clone());
            if !compacted && fact.usage.total_tokens > 0 {
                self.view.context.anchor = Some(crate::UsageAnchor {
                    task_material_tokens: self.view.context.task_material_tokens,
                    fact,
                    covered_items: coverage,
                });
            }
        }
        self.view.context.refresh(
            self.history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        self.confirm_history_commit(sequence);
        self.state.response_sequence = Some(sequence);
        Ok(result.compaction)
    }

    pub fn confirm_response_closed(
        &mut self,
        response_sequence: u64,
        sequence: u64,
    ) -> Result<(), ConversationFailure> {
        if self.state.response_sequence != Some(response_sequence) || self.has_open_calls() {
            return Err(ConversationFailure::new("批次关闭回执与当前响应不一致"));
        }
        self.state.response_sequence = None;
        self.confirm_history_commit(sequence);
        Ok(())
    }

    pub fn result_append(
        &self,
        call_id: &str,
        saved_sequence: u64,
    ) -> Result<HistoryAppend, ConversationFailure> {
        if self.results.get(call_id).and_then(|entry| entry.saved) != Some(saved_sequence) {
            return Err(ConversationFailure::new("工具结果追加缺少已确认的载荷"));
        }
        self.history
            .result_append(call_id)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))
    }

    pub fn next_saved_result(&self) -> Option<(String, u64)> {
        self.results
            .iter()
            .filter_map(|(id, entry)| entry.saved.map(|sequence| (id, sequence)))
            .min_by_key(|(_, sequence)| *sequence)
            .map(|(id, sequence)| (id.clone(), sequence))
    }

    pub fn confirm_result_append(
        &mut self,
        call_id: String,
        saved_sequence: u64,
        append: HistoryAppend,
        sequence: u64,
    ) -> Result<(), ConversationFailure> {
        if self.result_append(&call_id, saved_sequence)? != append {
            return Err(ConversationFailure::new("结果历史位置与回执不一致"));
        }
        let output = self
            .results
            .remove(&call_id)
            .ok_or_else(|| ConversationFailure::new("确认追加时工具结果已丢失"))?;
        self.tasks.append_result(&call_id, saved_sequence)?;
        self.history
            .append_result(call_id, output.output)
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        self.view.committed_items = self.history.summary();
        self.view.context.refresh(
            self.history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        self.confirm_history_commit(sequence);
        Ok(())
    }

    pub fn finish(&mut self, status: TurnStatus) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        for tool in &mut turn.tools {
            if matches!(tool.status, ToolStatus::Receiving | ToolStatus::Pending) {
                let result = ToolResult::not_executed(
                    "incomplete_response",
                    "模型响应未完成或调用未接纳，工具未执行",
                );
                tool.status = result.status;
                tool.output = Some(result.output.summary().to_owned());
            }
        }
        for input in &mut turn.steers {
            if matches!(input.state, SteerState::Accepted) {
                input.state = SteerState::NotApplied;
            }
        }
        self.state.completion = Some(status);
        self.view.context.refresh(
            self.history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        Ok(())
    }
}
