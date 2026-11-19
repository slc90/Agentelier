use crate::control::{MAX_PENDING_STEERS, SteerInput, SteerState, SteerView};
use crate::conversation::{
    ConversationFailure, SessionView, TaskPhase, ToolView, TurnStatus, TurnView,
};
use agentelier_llm_model::{
    FunctionCall, GenerationRequest, GenerationResult, OutputText, ProtocolHistory, StreamEvent,
};
use agentelier_tools::{
    MAX_SESSION_IMAGE_BYTES, MAX_TOOL_BODY_BYTES, ToolCatalog, ToolPayload, ToolResult, ToolStatus,
};
use std::sync::Arc;

pub struct ActiveTurn {
    pub(crate) memory: Option<Arc<agentelier_llm_model::MemorySnapshot>>,
    pub(crate) source_sequence: Option<u64>,
    pub(crate) history_confirmed: bool,
    pub(crate) view: SessionView,
    pub(crate) history: ProtocolHistory,
    pub(crate) tools: Arc<ToolCatalog>,
    pub(crate) input_retained: bool,
    pub(crate) results: std::collections::BTreeMap<String, ToolPayload>,
}
impl ActiveTurn {
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
                    self.memory
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
        crate::StoredState::from(&self.view)
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

    pub fn tool_user_turn(&self) -> Result<agentelier_tools::ToolUserTurn, ConversationFailure> {
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
            session: self.view.id,
            turn: turn.id.0,
            input: turn.input_id.0,
            statements,
        })
    }

    pub fn view(&self) -> SessionView {
        self.view.clone()
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
        let context = if !self.input_retained {
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
            memory: self.memory.as_deref().cloned(),
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

    pub fn update(&mut self, event: StreamEvent) -> Result<SessionView, ConversationFailure> {
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
                Self::upsert_tool(turn, call, ToolStatus::Receiving, &self.tools)?;
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
        Ok(self.view.clone())
    }

    pub fn accept_steer(&mut self, input: SteerInput) -> Result<(), ConversationFailure> {
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
        if !self.input_retained {
            self.history = self.history.with_user(&turn.input);
            self.input_retained = true;
        }
        let mut applied = Vec::new();
        for input in &mut turn.steers {
            if matches!(input.state, SteerState::Accepted) {
                self.history = self.history.with_user(&input.text);
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
    ) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        if result.calls.iter().any(|call| {
            turn.tools
                .iter()
                .any(|tool| tool.request_index < turn.request_index && tool.call_id == call.call_id)
        }) {
            return Err(ConversationFailure::new("任务重复使用已有 call_id"));
        }
        turn.phase = TaskPhase::RequestingModel;
        turn.messages
            .retain(|message| message.request_index != turn.request_index);
        turn.messages
            .extend(result.messages.iter().cloned().map(|mut message| {
                message.request_index = turn.request_index;
                message
            }));
        for call in &result.calls {
            Self::upsert_tool(turn, call.clone(), ToolStatus::Pending, &self.tools)?;
        }
        Ok(())
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
    ) -> Result<(ToolStatus, ToolPayload), ConversationFailure> {
        if result.output.image().is_some()
            && self
                .history
                .image_bytes_with(self.results.values().chain(std::iter::once(&result.output)))
                > MAX_SESSION_IMAGE_BYTES
        {
            result = result.reject_image_budget();
        }
        if self.results.contains_key(call_id) {
            return Err(ConversationFailure::new("工具结果身份重复"));
        }
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == turn.request_index && tool.call_id == call_id)
            .ok_or(ConversationFailure::new("工具结果缺少调用记录"))?;
        tool.status = result.status;
        tool.output = Some(result.output.summary().to_owned());
        self.results
            .insert(call_id.to_owned(), result.output.clone());
        Ok((result.status, result.output))
    }

    pub fn seal(&mut self, result: GenerationResult) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure::new("活动任务缺少轮次"))?;
        let results = result
            .calls
            .iter()
            .map(|call| {
                let output = self
                    .results
                    .get(&call.call_id)
                    .cloned()
                    .ok_or(ConversationFailure::new("工具步骤尚未封闭"))?;
                Ok((call.call_id.clone(), output))
            })
            .collect::<Result<Vec<_>, ConversationFailure>>()?;
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
        self.history
            .seal(
                (!self.input_retained).then_some(turn.input.as_str()),
                result.output,
                results,
            )
            .map_err(|_| ConversationFailure::new("工具历史关系损坏"))?;
        self.input_retained = true;
        self.results.clear();
        turn.committed_steps += 1;
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
            &self.history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
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
        turn.status = status;
        self.view.context.refresh(
            &self.history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        Ok(())
    }
}
