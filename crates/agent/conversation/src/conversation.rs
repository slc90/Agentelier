use agentelier_llm_model::{
    GenerationRequest, GenerationResult, ModelFailure, OutputMessage, OutputText, StreamEvent,
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
    Send { session: SessionId, input: String },
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
    pub input: String,
    pub messages: Vec<OutputMessage>,
    pub status: TurnStatus,
}
#[derive(Clone)]
pub struct SessionView {
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<TurnView>,
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

#[derive(Default)]
pub struct Conversation {
    session: Option<SessionView>,
    next_session: u64,
    next_turn: u64,
}

impl Conversation {
    pub fn synchronize_account(&mut self, subject: Option<&str>) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| Some(session.settings.account_subject.as_str()) != subject)
        {
            self.session = None;
        }
    }

    pub fn create(
        &mut self,
        settings: SessionSettings,
    ) -> Result<SessionView, ConversationFailure> {
        if self.session.is_some() {
            return Err(ConversationFailure("当前聊天会话已建立"));
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
        };
        self.next_session = id;
        self.session = Some(session.clone());
        Ok(session)
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
        if session.id != session_id {
            return Err(ConversationFailure("聊天会话身份已变化"));
        }
        if session.turn.is_some() {
            return Err(ConversationFailure("当前会话已有首轮记录"));
        }
        let id = self
            .next_turn
            .checked_add(1)
            .ok_or(ConversationFailure("聊天轮次标识已耗尽"))?;
        let request = GenerationRequest {
            slug: session.settings.slug.clone(),
            effort: session.settings.effort.clone(),
            input: input.clone(),
            account_subject: session.settings.account_subject.clone(),
        };
        session.turn = Some(TurnView {
            id: TurnId(id),
            operation_id,
            input,
            messages: Vec::new(),
            status: TurnStatus::Running,
        });
        self.next_turn = id;
        Ok((session.clone(), request))
    }

    fn turn(&mut self, operation_id: u64) -> Result<&mut TurnView, ConversationFailure> {
        self.session
            .as_mut()
            .and_then(|session| session.turn.as_mut())
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
            .clone()
            .ok_or(ConversationFailure("运行轮次缺少会话"))
    }

    pub fn finish(
        &mut self,
        operation_id: u64,
        outcome: Option<Result<GenerationResult, ModelFailure>>,
    ) -> Result<SessionView, ConversationFailure> {
        let turn = self.turn(operation_id)?;
        turn.status = match outcome {
            Some(Ok(result)) => {
                turn.messages = result.messages;
                TurnStatus::Completed
            }
            Some(Err(failure)) => TurnStatus::Failed(failure.to_string()),
            None => TurnStatus::Cancelled,
        };
        self.session
            .clone()
            .ok_or(ConversationFailure("完成轮次缺少会话"))
    }
}
