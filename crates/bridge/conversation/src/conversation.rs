#[derive(Clone)]
pub struct SessionSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub account_subject: String,
}
pub enum ConversationRequest {
    Create(SessionSettings),
    Reset { session: u64 },
    Send { session: u64, input: String },
    Retry { session: u64, turn: u64 },
}
#[derive(Clone)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
}
#[derive(Clone)]
pub enum HistoryItem {
    User,
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone)]
pub struct MessageView {
    pub id: String,
    pub output_index: usize,
    pub phase: Option<String>,
    pub text: String,
}
#[derive(Clone)]
pub struct TurnView {
    pub id: u64,
    pub operation_id: u64,
    pub input_id: u64,
    pub is_retry: bool,
    pub input: String,
    pub messages: Vec<MessageView>,
    pub status: TurnStatus,
    pub request_items: Vec<HistoryItem>,
}
#[derive(Clone)]
pub struct SessionView {
    pub id: u64,
    pub settings: SessionSettings,
    pub turn: Option<TurnView>,
    pub previous_turns: Vec<TurnView>,
    pub committed_items: Vec<HistoryItem>,
}
pub enum ConversationEvent {
    Created(SessionView),
    Accepted(SessionView),
    Updated(SessionView),
    Finished(SessionView),
    Rejected(String),
}

impl ConversationEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Created(_) | Self::Finished(_) | Self::Rejected(_)
        )
    }
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Updated(_))
    }
}

pub fn request(request: ConversationRequest) -> agentelier_conversation::ConversationRequest {
    match request {
        ConversationRequest::Create(settings) => {
            agentelier_conversation::ConversationRequest::Create(
                agentelier_conversation::SessionSettings {
                    slug: settings.slug,
                    display_name: settings.display_name,
                    effort: settings.effort,
                    account_subject: settings.account_subject,
                },
            )
        }
        ConversationRequest::Reset { session } => {
            agentelier_conversation::ConversationRequest::Reset {
                session: agentelier_conversation::SessionId(session),
            }
        }
        ConversationRequest::Retry { session, turn } => {
            agentelier_conversation::ConversationRequest::Retry {
                session: agentelier_conversation::SessionId(session),
                turn: agentelier_conversation::TurnId(turn),
            }
        }
        ConversationRequest::Send { session, input } => {
            agentelier_conversation::ConversationRequest::Send {
                session: agentelier_conversation::SessionId(session),
                input,
            }
        }
    }
}

fn view(session: agentelier_conversation::SessionView) -> SessionView {
    SessionView {
        id: session.id.0,
        settings: SessionSettings {
            slug: session.settings.slug,
            display_name: session.settings.display_name,
            effort: session.settings.effort,
            account_subject: session.settings.account_subject,
        },
        turn: session.turn.map(turn_view),
        previous_turns: session.previous_turns.into_iter().map(turn_view).collect(),
        committed_items: session
            .committed_items
            .into_iter()
            .map(history_item)
            .collect(),
    }
}

fn history_item(item: agentelier_conversation::ProtocolItemSummary) -> HistoryItem {
    match item {
        agentelier_conversation::ProtocolItemSummary::User => HistoryItem::User,
        agentelier_conversation::ProtocolItemSummary::Assistant { phase } => {
            HistoryItem::Assistant { phase }
        }
        agentelier_conversation::ProtocolItemSummary::Reasoning { encrypted } => {
            HistoryItem::Reasoning { encrypted }
        }
    }
}
fn turn_view(turn: agentelier_conversation::TurnView) -> TurnView {
    TurnView {
        id: turn.id.0,
        input_id: turn.input_id.0,
        is_retry: turn.is_retry,
        operation_id: turn.operation_id,
        input: turn.input,
        messages: turn
            .messages
            .into_iter()
            .map(|message| MessageView {
                id: message.id,
                output_index: message.output_index,
                phase: message.phase,
                text: message
                    .content
                    .into_iter()
                    .map(|part| part.text)
                    .collect::<Vec<_>>()
                    .join("\n\n"),
            })
            .collect(),
        request_items: turn.request_items.into_iter().map(history_item).collect(),
        status: match turn.status {
            agentelier_conversation::TurnStatus::Running => TurnStatus::Running,
            agentelier_conversation::TurnStatus::Completed => TurnStatus::Completed,
            agentelier_conversation::TurnStatus::Cancelled => TurnStatus::Cancelled,
            agentelier_conversation::TurnStatus::Failed(reason) => TurnStatus::Failed(reason),
        },
    }
}

pub fn convert(event: agentelier_conversation::ConversationEvent) -> ConversationEvent {
    match event {
        agentelier_conversation::ConversationEvent::Created(session) => {
            ConversationEvent::Created(view(session))
        }
        agentelier_conversation::ConversationEvent::Accepted(session) => {
            ConversationEvent::Accepted(view(session))
        }
        agentelier_conversation::ConversationEvent::Updated(session) => {
            ConversationEvent::Updated(view(session))
        }
        agentelier_conversation::ConversationEvent::Finished(session) => {
            ConversationEvent::Finished(view(session))
        }
        agentelier_conversation::ConversationEvent::Rejected(reason) => {
            ConversationEvent::Rejected(reason)
        }
    }
}
