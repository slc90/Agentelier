use crate::control::{MAX_PENDING_STEERS, SteerInput, SteerState, SteerView};
use crate::conversation::{
    ConversationFailure, SessionView, TaskPhase, ToolView, TurnStatus, TurnView,
};
use agentelier_llm_model::{
    FunctionCall, GenerationRequest, GenerationResult, OutputText, ProtocolHistory, StreamEvent,
};
use agentelier_tools::{MAX_TOOL_BODY_BYTES, ToolCatalog, ToolResult, ToolStatus};
use std::sync::Arc;

pub struct ActiveTurn {
    pub(crate) view: SessionView,
    pub(crate) history: ProtocolHistory,
    pub(crate) tools: Arc<ToolCatalog>,
    pub(crate) input_retained: bool,
}
impl ActiveTurn {
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
        Ok(GenerationRequest {
            slug: self.view.settings.slug.clone(),
            effort: self.view.settings.effort.clone(),
            context,
            tools: self.tools.clone(),
            account_subject: self.view.settings.account_subject.clone(),
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
            StreamEvent::FunctionCall(call) => {
                Self::upsert_tool(turn, call, ToolStatus::Receiving)?;
            }
            StreamEvent::Item(mut message) => {
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
        turn.messages
            .retain(|message| message.request_index != turn.request_index);
        turn.messages
            .extend(result.messages.iter().cloned().map(|mut message| {
                message.request_index = turn.request_index;
                message
            }));
        for call in &result.calls {
            Self::upsert_tool(turn, call.clone(), ToolStatus::Pending)?;
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
        result: ToolResult,
    ) -> Result<(), ConversationFailure> {
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
        tool.output = Some(result.output);
        Ok(())
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
                let output = turn
                    .tools
                    .iter()
                    .find(|tool| {
                        tool.request_index == turn.request_index && tool.call_id == call.call_id
                    })
                    .and_then(|tool| tool.output.clone())
                    .ok_or(ConversationFailure::new("工具步骤尚未封闭"))?;
                Ok((call.call_id.clone(), output))
            })
            .collect::<Result<Vec<_>, ConversationFailure>>()?;
        self.history
            .seal(
                (!self.input_retained).then_some(turn.input.as_str()),
                result.output,
                results,
            )
            .map_err(|_| ConversationFailure::new("工具历史关系损坏"))?;
        self.input_retained = true;
        turn.committed_steps += 1;
        for tool in &mut turn.tools {
            if tool.request_index == turn.request_index {
                tool.retained = true;
            }
        }
        self.view.committed_items = self.history.summary();
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
                tool.output = Some(result.output);
            }
        }
        for input in &mut turn.steers {
            if matches!(input.state, SteerState::Accepted) {
                input.state = SteerState::NotApplied;
            }
        }
        turn.status = status;
        Ok(())
    }
}
