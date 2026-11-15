use agentelier_llm_model::{
    FunctionCall, GenerationRequest, GenerationResult, OutputMessage, OutputText, ProtocolHistory,
    ProtocolItemSummary, StreamEvent,
};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use agentelier_tools::{MAX_TOOL_BODY_BYTES, ToolCatalog, ToolResult, ToolStatus};
use std::sync::Arc;
use std::{error::Error, fmt};

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SessionId(pub u64);
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TurnId(pub u64);

#[derive(Clone)]
pub struct SessionSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub account_subject: String,
    pub instructions: Arc<InstructionSnapshot>,
}
pub enum ConversationRequest {
    Create(SessionSettings),
    Reset { session: SessionId },
    Send { session: SessionId, input: String },
    Retry { session: SessionId, turn: TurnId },
}
#[derive(Clone)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
    Limited,
}
#[derive(Clone, Copy)]
pub enum TaskPhase {
    RequestingModel,
    ExecutingTool,
}
#[derive(Clone)]
pub struct ToolView {
    pub request_index: u32,
    pub id: String,
    pub output_index: usize,
    pub call_id: String,
    pub name: String,
    pub arguments: String,
    pub arguments_truncated: bool,
    pub status: ToolStatus,
    pub output: Option<String>,
    pub retained: bool,
}
#[derive(Clone)]
pub struct TurnView {
    pub id: TurnId,
    pub operation_id: u64,
    pub input_id: TurnId,
    pub is_retry: bool,
    pub input: String,
    pub messages: Vec<OutputMessage>,
    pub status: TurnStatus,
    pub request_items: Vec<ProtocolItemSummary>,
    pub request_index: u32,
    pub committed_steps: u32,
    pub tools: Vec<ToolView>,
    pub phase: TaskPhase,
    pub request_instructions: Vec<InstructionMessage>,
}
#[derive(Clone)]
pub struct SessionView {
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<Box<TurnView>>,
    pub previous_turns: Vec<TurnView>,
    pub committed_items: Vec<ProtocolItemSummary>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
}
pub enum ConversationEvent {
    Created(SessionView),
    Accepted(SessionView),
    Updated(SessionView),
    Finished(SessionView),
    Rejected(String),
}

#[derive(Debug)]
pub struct ConversationFailure(&'static str);
impl ConversationFailure {
    pub fn new(message: &'static str) -> Self {
        Self(message)
    }
}
impl fmt::Display for ConversationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for ConversationFailure {}

struct ChatSession {
    view: SessionView,
    history: Option<ProtocolHistory>,
    tools: Arc<ToolCatalog>,
}

#[derive(Default)]
pub struct Conversation {
    session: Option<ChatSession>,
    next_session: u64,
    next_turn: u64,
}

impl Conversation {
    pub fn synchronize_account(&mut self, subject: Option<&str>) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| Some(session.view.settings.account_subject.as_str()) != subject)
        {
            self.session = None;
        }
    }

    pub fn create(
        &mut self,
        settings: SessionSettings,
    ) -> Result<SessionView, ConversationFailure> {
        if self
            .session
            .as_ref()
            .and_then(|current| current.view.turn.as_ref())
            .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
        {
            return Err(ConversationFailure("生成尚未结束，不能替换会话"));
        }
        if settings.slug.trim().is_empty()
            || settings.account_subject.is_empty()
            || settings
                .effort
                .as_ref()
                .is_some_and(|effort| effort.trim().is_empty())
        {
            return Err(ConversationFailure(
                "会话设置缺少有效模型、推理选项或账号身份",
            ));
        }
        let id = self
            .next_session
            .checked_add(1)
            .ok_or(ConversationFailure("聊天会话标识已耗尽"))?;
        settings
            .instructions
            .validate()
            .map_err(|_| ConversationFailure("会话指令快照无效"))?;
        let tools =
            Arc::new(ToolCatalog::standard().map_err(|_| ConversationFailure("工具目录配置无效"))?);
        let session = SessionView {
            id: SessionId(id),
            settings,
            turn: None,
            previous_turns: Vec::new(),
            committed_items: Vec::new(),
            execution_blocked: false,
            tool_catalog: tools
                .definitions()
                .iter()
                .map(|tool| (tool.name.to_owned(), tool.description.to_owned()))
                .collect(),
        };
        self.next_session = id;
        self.session = Some(ChatSession {
            view: session.clone(),
            history: Some(ProtocolHistory::default()),
            tools,
        });
        Ok(session)
    }

    pub fn reset(&mut self, id: SessionId) -> Result<SessionView, ConversationFailure> {
        let settings = self
            .session
            .as_ref()
            .filter(|current| current.view.id == id)
            .ok_or(ConversationFailure("要重置的会话已变化"))?
            .view
            .settings
            .clone();
        self.create(settings)
    }

    pub fn begin(
        &mut self,
        operation_id: u64,
        session_id: SessionId,
        input: String,
    ) -> Result<(SessionView, ActiveTurn), ConversationFailure> {
        if input.trim().is_empty() {
            return Err(ConversationFailure("不能发送空白消息"));
        }
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure("尚未创建聊天会话"))?;
        if session.view.id != session_id {
            return Err(ConversationFailure("聊天会话身份已变化"));
        }
        if session
            .view
            .turn
            .as_ref()
            .is_some_and(|turn| matches!(turn.status, TurnStatus::Running))
        {
            return Err(ConversationFailure("上一轮尚未结束"));
        }
        let id = self
            .next_turn
            .checked_add(1)
            .ok_or(ConversationFailure("聊天轮次标识已耗尽"))?;
        if session.view.execution_blocked || session.history.is_none() {
            return Err(ConversationFailure("会话执行记录不可用，请重置或新建会话"));
        }
        let request_items = session
            .history
            .as_ref()
            .ok_or(ConversationFailure("会话缺少历史"))?
            .with_user(&input)
            .summary();
        if let Some(previous) = session.view.turn.take() {
            session.view.previous_turns.push(*previous);
        }
        session.view.turn = Some(Box::new(TurnView {
            id: TurnId(id),
            input_id: TurnId(id),
            is_retry: false,
            operation_id,
            input,
            messages: Vec::new(),
            status: TurnStatus::Running,
            request_items,
            request_index: 0,
            committed_steps: 0,
            tools: Vec::new(),
            phase: TaskPhase::RequestingModel,
            request_instructions: session.view.settings.instructions.messages(),
        }));
        self.next_turn = id;
        let active = ActiveTurn {
            view: session.view.clone(),
            tools: session.tools.clone(),
            history: session
                .history
                .take()
                .ok_or(ConversationFailure("会话缺少历史"))?,
        };
        Ok((session.view.clone(), active))
    }

    pub fn start(
        &mut self,
        operation: u64,
        request: ConversationRequest,
    ) -> Result<(SessionView, ActiveTurn), ConversationFailure> {
        match request {
            ConversationRequest::Send { session, input } => self.begin(operation, session, input),
            ConversationRequest::Retry { session, turn } => {
                let previous = self
                    .session
                    .as_ref()
                    .filter(|current| current.view.id == session)
                    .and_then(|current| current.view.turn.as_ref())
                    .filter(|previous| {
                        previous.id == turn
                            && previous.committed_steps == 0
                            && matches!(
                                previous.status,
                                TurnStatus::Failed(_) | TurnStatus::Cancelled
                            )
                    })
                    .ok_or(ConversationFailure("只有最近失败或取消的输入可以重试"))?;
                let input = previous.input.clone();
                let input_id = previous.input_id;
                let (_, mut active) = self.begin(operation, session, input)?;
                let current = self
                    .session
                    .as_mut()
                    .ok_or(ConversationFailure("重试缺少会话"))?;
                let turn = current
                    .view
                    .turn
                    .as_mut()
                    .ok_or(ConversationFailure("重试缺少轮次"))?;
                turn.input_id = input_id;
                turn.is_retry = true;
                active.view = current.view.clone();
                Ok((current.view.clone(), active))
            }
            ConversationRequest::Create(_) | ConversationRequest::Reset { .. } => {
                Err(ConversationFailure("创建会话不是生成操作"))
            }
        }
    }

    fn turn(&mut self, operation_id: u64) -> Result<&mut TurnView, ConversationFailure> {
        self.session
            .as_mut()
            .and_then(|session| session.view.turn.as_deref_mut())
            .filter(|turn| {
                turn.operation_id == operation_id && matches!(turn.status, TurnStatus::Running)
            })
            .ok_or(ConversationFailure("模型结果不属于当前运行轮次"))
    }

    pub fn update(
        &mut self,
        operation_id: u64,
        view: SessionView,
    ) -> Result<SessionView, ConversationFailure> {
        let turn = self.turn(operation_id)?;
        if view
            .turn
            .as_ref()
            .is_none_or(|incoming| incoming.id != turn.id || incoming.operation_id != operation_id)
            || self
                .session
                .as_ref()
                .is_none_or(|session| session.view.id != view.id)
        {
            return Err(ConversationFailure("任务快照身份不匹配"));
        }
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure("任务缺少会话"))?;
        session.view = view;
        Ok(session.view.clone())
    }

    pub fn finish(
        &mut self,
        operation_id: u64,
        active: ActiveTurn,
    ) -> Result<SessionView, ConversationFailure> {
        let view = self.update(operation_id, active.view)?;
        self.session
            .as_mut()
            .ok_or(ConversationFailure("任务缺少会话"))?
            .history = Some(active.history);
        Ok(view)
    }

    pub fn interrupt(&mut self, operation_id: u64) -> Result<SessionView, ConversationFailure> {
        self.turn(operation_id)?.status =
            TurnStatus::Failed("任务异常结束，无法归还执行记录，请重置或新建会话".into());
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure("任务缺少会话"))?;
        session.view.execution_blocked = true;
        Ok(session.view.clone())
    }
}

pub struct ActiveTurn {
    view: SessionView,
    history: ProtocolHistory,
    tools: Arc<ToolCatalog>,
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
        turn.request_index = turn
            .request_index
            .checked_add(1)
            .ok_or(ConversationFailure("模型请求序号已耗尽"))?;
        turn.phase = TaskPhase::RequestingModel;
        let context = if turn.committed_steps == 0 {
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
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
                        return Err(ConversationFailure("模型输出项身份不一致"));
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
                    .ok_or(ConversationFailure("正文片段缺少对应输出项"))?;
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
                    .ok_or(ConversationFailure("文本增量缺少对应输出项"))?;
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
                return Err(ConversationFailure("工具输出身份冲突"));
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
        if result.calls.iter().any(|call| {
            turn.tools
                .iter()
                .any(|tool| tool.request_index < turn.request_index && tool.call_id == call.call_id)
        }) {
            return Err(ConversationFailure("任务重复使用已有 call_id"));
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
        turn.phase = TaskPhase::ExecutingTool;
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == turn.request_index && tool.call_id == call_id)
            .ok_or(ConversationFailure("执行工具缺少调用记录"))?;
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
        let tool = turn
            .tools
            .iter_mut()
            .find(|tool| tool.request_index == turn.request_index && tool.call_id == call_id)
            .ok_or(ConversationFailure("工具结果缺少调用记录"))?;
        tool.status = result.status;
        tool.output = Some(result.output);
        Ok(())
    }

    pub fn seal(&mut self, result: GenerationResult) -> Result<(), ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
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
                    .ok_or(ConversationFailure("工具步骤尚未封闭"))?;
                Ok((call.call_id.clone(), output))
            })
            .collect::<Result<Vec<_>, ConversationFailure>>()?;
        self.history
            .seal(
                (turn.committed_steps == 0).then_some(turn.input.as_str()),
                result.output,
                results,
            )
            .map_err(|_| ConversationFailure("工具历史关系损坏"))?;
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
            .ok_or(ConversationFailure("活动任务缺少轮次"))?;
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
        turn.status = status;
        Ok(())
    }
}
