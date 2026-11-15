use crate::active::ActiveTurn;
use crate::control::SteerView;
use agentelier_llm_model::{OutputMessage, ProtocolHistory, ProtocolItemSummary};
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use agentelier_tools::{ToolCatalog, ToolStatus};
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
    pub steers: Vec<SteerView>,
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
    InputReceipt(crate::control::InputReceipt),
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
            steers: Vec::new(),
            request_instructions: session.view.settings.instructions.messages(),
        }));
        self.next_turn = id;
        let active = ActiveTurn {
            view: session.view.clone(),
            tools: session.tools.clone(),
            input_retained: false,
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
                            && !previous.steers.iter().any(|input| {
                                matches!(input.state, crate::control::SteerState::Applied)
                            })
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
