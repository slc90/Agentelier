use agentelier_llm_model::{
    GenerationRequest, GenerationResult, ModelFailure, OutputMessage, OutputText, ProtocolHistory,
    ProtocolItemSummary, StreamEvent,
};
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
}
#[derive(Clone)]
pub struct SessionView {
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<TurnView>,
    pub previous_turns: Vec<TurnView>,
    pub committed_items: Vec<ProtocolItemSummary>,
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
impl fmt::Display for ConversationFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}
impl Error for ConversationFailure {}

struct ChatSession {
    view: SessionView,
    history: ProtocolHistory,
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
        let session = SessionView {
            id: SessionId(id),
            settings,
            turn: None,
            previous_turns: Vec::new(),
            committed_items: Vec::new(),
        };
        self.next_session = id;
        self.session = Some(ChatSession {
            view: session.clone(),
            history: ProtocolHistory::default(),
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
    ) -> Result<(SessionView, GenerationRequest), ConversationFailure> {
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
        let context = session.history.with_user(&input);
        let request_items = context.summary();
        let request = GenerationRequest {
            slug: session.view.settings.slug.clone(),
            effort: session.view.settings.effort.clone(),
            context,
            account_subject: session.view.settings.account_subject.clone(),
        };
        if let Some(previous) = session.view.turn.take() {
            session.view.previous_turns.push(previous);
        }
        session.view.turn = Some(TurnView {
            id: TurnId(id),
            input_id: TurnId(id),
            is_retry: false,
            operation_id,
            input,
            messages: Vec::new(),
            status: TurnStatus::Running,
            request_items,
        });
        self.next_turn = id;
        Ok((session.view.clone(), request))
    }

    pub fn start(
        &mut self,
        operation: u64,
        request: ConversationRequest,
    ) -> Result<(SessionView, GenerationRequest), ConversationFailure> {
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
                            && matches!(
                                previous.status,
                                TurnStatus::Failed(_) | TurnStatus::Cancelled
                            )
                    })
                    .ok_or(ConversationFailure("只有最近失败或取消的输入可以重试"))?;
                let input = previous.input.clone();
                let input_id = previous.input_id;
                let (_, request) = self.begin(operation, session, input)?;
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
                Ok((current.view.clone(), request))
            }
            ConversationRequest::Create(_) | ConversationRequest::Reset { .. } => {
                Err(ConversationFailure("创建会话不是生成操作"))
            }
        }
    }

    fn turn(&mut self, operation_id: u64) -> Result<&mut TurnView, ConversationFailure> {
        self.session
            .as_mut()
            .and_then(|session| session.view.turn.as_mut())
            .filter(|turn| {
                turn.operation_id == operation_id && matches!(turn.status, TurnStatus::Running)
            })
            .ok_or(ConversationFailure("模型结果不属于当前运行轮次"))
    }

    pub fn update(
        &mut self,
        operation_id: u64,
        event: StreamEvent,
    ) -> Result<SessionView, ConversationFailure> {
        let turn = self.turn(operation_id)?;
        match event {
            StreamEvent::Item(message) => {
                if let Some(existing) = turn
                    .messages
                    .iter_mut()
                    .find(|existing| existing.output_index == message.output_index)
                {
                    if existing.id != message.id {
                        return Err(ConversationFailure("模型输出项身份不一致"));
                    }
                    *existing = message;
                } else {
                    turn.messages.push(message);
                }
                turn.messages.sort_by_key(|message| message.output_index);
            }
            StreamEvent::Part {
                item_id,
                output_index,
                part,
            } => {
                let message = turn
                    .messages
                    .iter_mut()
                    .find(|message| message.output_index == output_index && message.id == item_id)
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
                    .find(|message| message.output_index == output_index && message.id == item_id)
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
        self.session
            .as_ref()
            .map(|session| session.view.clone())
            .ok_or(ConversationFailure("运行轮次缺少会话"))
    }

    pub fn finish(
        &mut self,
        operation_id: u64,
        outcome: Option<Result<GenerationResult, ModelFailure>>,
    ) -> Result<SessionView, ConversationFailure> {
        self.turn(operation_id)?;
        let session = self
            .session
            .as_mut()
            .ok_or(ConversationFailure("完成轮次缺少会话"))?;
        let turn = session
            .view
            .turn
            .as_mut()
            .ok_or(ConversationFailure("完成结果缺少对应轮次"))?;
        turn.status = match outcome {
            Some(Ok(result)) => {
                session.history.commit(&turn.input, result.output);
                session.view.committed_items = session.history.summary();
                turn.messages = result.messages;
                TurnStatus::Completed
            }
            Some(Err(failure)) => TurnStatus::Failed(failure.to_string()),
            None => TurnStatus::Cancelled,
        };
        Ok(session.view.clone())
    }
}
