pub use agentelier_conversation::SessionId;
use agentelier_prompts::{InstructionMessage, InstructionSnapshot};
use std::sync::Arc;

#[derive(Clone)]
pub struct SessionSettings {
    pub slug: String,
    pub display_name: String,
    pub effort: Option<String>,
    pub image_input: Option<bool>,
    pub account_subject: String,
    pub instructions: Arc<InstructionSnapshot>,
    pub working_directory: std::path::PathBuf,
}
pub enum ConversationRequest {
    Open { session: SessionId },
    Create(SessionSettings),
    Reset { session: SessionId },
    Send { session: SessionId, input: String },
    Retry { session: SessionId, turn: u64 },
}
#[derive(Clone)]
pub enum TurnStatus {
    Running,
    Completed,
    Cancelled,
    Failed(String),
    Limited,
}
#[derive(Clone)]
pub enum HistoryItem {
    User,
    FunctionCall { call_id: String, name: String },
    FunctionResult { call_id: String },
    Assistant { phase: Option<String> },
    Reasoning { encrypted: bool },
}
#[derive(Clone)]
pub struct MessageView {
    pub id: String,
    pub output_index: usize,
    pub request_index: u32,
    pub phase: Option<String>,
    pub text: String,
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
    pub status: String,
    pub output: Option<String>,
    pub retained: bool,
}
#[derive(Clone, Copy)]
pub enum SteerState {
    Accepted,
    Applied,
    NotApplied,
}
#[derive(Clone)]
pub struct SteerView {
    pub id: u64,
    pub text: String,
    pub state: SteerState,
    pub applied_request: Option<u32>,
}
pub enum InputDecision {
    Accepted,
    Applied,
    Rejected(String),
}
pub struct InputReceipt {
    pub id: u64,
    pub decision: InputDecision,
}
#[derive(Clone)]
pub struct ProcessCleanup {
    pub session_id: String,
    pub cleanup_confirmed: bool,
    pub output: String,
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
    pub request_index: u32,
    pub committed_steps: u32,
    pub tools: Vec<ToolView>,
    pub phase: String,
    pub steers: Vec<SteerView>,
    pub request_instructions: Vec<InstructionMessage>,
    pub process_cleanup: Vec<ProcessCleanup>,
}
#[derive(Clone)]
pub struct QueuedInput {
    pub operation_id: u64,
    pub input_id: u64,
    pub text: String,
}
#[derive(Clone)]
pub enum SessionAction {
    Enqueue { session: SessionId, text: String },
    Remove { session: SessionId, operation: u64 },
    Resume { session: SessionId },
}
pub enum QueueDecision {
    Enqueued,
    Removed { operation: u64 },
    AlreadyStarted { operation: u64 },
    Resumed,
    Rejected(String),
}
pub struct QueueEvent {
    pub id: u64,
    pub view: Option<SessionView>,
    pub decision: QueueDecision,
}
#[derive(Clone)]
pub struct SessionView {
    pub notice: Option<String>,
    pub id: SessionId,
    pub settings: SessionSettings,
    pub turn: Option<Box<TurnView>>,
    pub previous_turns: Vec<TurnView>,
    pub committed_items: Vec<HistoryItem>,
    pub execution_blocked: bool,
    pub tool_catalog: Vec<(String, String)>,
    pub queue: Vec<QueuedInput>,
    pub queue_paused: bool,
}
pub enum ConversationEvent {
    Archives(Vec<ArchiveEntry>),
    Opened(SessionView),
    ArchiveFailed(String),
    Created(SessionView),
    Accepted(SessionView),
    Updated(SessionView),
    Finished(SessionView),
    Rejected(String),
    InputReceipt(InputReceipt),
    Queue(QueueEvent),
    Started(SessionView),
}

#[derive(Clone)]
pub struct ArchiveEntry {
    pub id: SessionId,
    pub title: String,
    pub created_at_ms: u64,
    pub last_activity_ms: u64,
    pub last_sequence: u64,
    pub error: Option<String>,
}

impl ConversationEvent {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Created(_)
                | Self::Finished(_)
                | Self::Rejected(_)
                | Self::Opened(_)
                | Self::ArchiveFailed(_)
        )
    }
    pub fn is_progress(&self) -> bool {
        matches!(self, Self::Updated(_))
    }
}

pub fn request(request: ConversationRequest) -> agentelier_conversation::ConversationRequest {
    match request {
        ConversationRequest::Open { session } => {
            agentelier_conversation::ConversationRequest::Open { session }
        }
        ConversationRequest::Create(settings) => {
            agentelier_conversation::ConversationRequest::Create(
                agentelier_conversation::SessionSettings {
                    slug: settings.slug,
                    display_name: settings.display_name,
                    effort: settings.effort,
                    image_input: settings.image_input,
                    account_subject: settings.account_subject,
                    instructions: settings.instructions,
                    working_directory: settings.working_directory,
                },
            )
        }
        ConversationRequest::Reset { session } => {
            agentelier_conversation::ConversationRequest::Reset { session }
        }
        ConversationRequest::Retry { session, turn } => {
            agentelier_conversation::ConversationRequest::Retry {
                session,
                turn: agentelier_conversation::TurnId(turn),
            }
        }
        ConversationRequest::Send { session, input } => {
            agentelier_conversation::ConversationRequest::Send { session, input }
        }
    }
}

fn view(session: agentelier_conversation::SessionView) -> SessionView {
    SessionView {
        notice: session.notice,
        execution_blocked: session.execution_blocked,
        tool_catalog: session.tool_catalog,
        queue: session
            .queue
            .into_iter()
            .map(|input| QueuedInput {
                operation_id: input.operation_id,
                input_id: input.input_id.0,
                text: input.text,
            })
            .collect(),
        queue_paused: session.queue_paused,
        id: session.id,
        settings: SessionSettings {
            slug: session.settings.slug,
            display_name: session.settings.display_name,
            effort: session.settings.effort,
            image_input: session.settings.image_input,
            account_subject: session.settings.account_subject,
            instructions: session.settings.instructions,
            working_directory: session.settings.working_directory,
        },
        turn: session.turn.map(|turn| Box::new(turn_view(*turn))),
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
        agentelier_conversation::ProtocolItemSummary::FunctionCall { call_id, name } => {
            HistoryItem::FunctionCall { call_id, name }
        }
        agentelier_conversation::ProtocolItemSummary::FunctionResult { call_id } => {
            HistoryItem::FunctionResult { call_id }
        }
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
        steers: turn
            .steers
            .into_iter()
            .map(|input| SteerView {
                id: input.id,
                text: input.text,
                applied_request: input.applied_request,
                state: match input.state {
                    agentelier_conversation::SteerState::Accepted => SteerState::Accepted,
                    agentelier_conversation::SteerState::Applied => SteerState::Applied,
                    agentelier_conversation::SteerState::NotApplied => SteerState::NotApplied,
                },
            })
            .collect(),
        request_index: turn.request_index,
        committed_steps: turn.committed_steps,
        phase: match turn.phase {
            agentelier_conversation::TaskPhase::RequestingModel => "model",
            agentelier_conversation::TaskPhase::ExecutingTool => "tool",
            agentelier_conversation::TaskPhase::ReapingProcesses => "cleanup",
        }
        .into(),
        tools: turn
            .tools
            .into_iter()
            .map(|tool| ToolView {
                request_index: tool.request_index,
                id: tool.id,
                output_index: tool.output_index,
                call_id: tool.call_id,
                name: tool.name,
                arguments: tool.arguments,
                arguments_truncated: tool.arguments_truncated,
                output: tool.output,
                retained: tool.retained,
                status: match tool.status {
                    agentelier_conversation::ToolStatus::Unconfirmed => "unconfirmed",
                    agentelier_conversation::ToolStatus::Receiving => "receiving",
                    agentelier_conversation::ToolStatus::Pending => "pending",
                    agentelier_conversation::ToolStatus::Running => "running",
                    agentelier_conversation::ToolStatus::Succeeded => "succeeded",
                    agentelier_conversation::ToolStatus::Failed => "failed",
                    agentelier_conversation::ToolStatus::TimedOut => "timed_out",
                    agentelier_conversation::ToolStatus::Cancelled => "cancelled",
                    agentelier_conversation::ToolStatus::NotExecuted => "not_executed",
                }
                .into(),
            })
            .collect(),
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
                request_index: message.request_index,
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
        request_instructions: turn.request_instructions,
        process_cleanup: turn
            .process_cleanup
            .into_iter()
            .map(|record| ProcessCleanup {
                session_id: record.session_id,
                cleanup_confirmed: record.cleanup_confirmed,
                output: record.output,
            })
            .collect(),
        status: match turn.status {
            agentelier_conversation::TurnStatus::Limited => TurnStatus::Limited,
            agentelier_conversation::TurnStatus::Running => TurnStatus::Running,
            agentelier_conversation::TurnStatus::Completed => TurnStatus::Completed,
            agentelier_conversation::TurnStatus::Cancelled => TurnStatus::Cancelled,
            agentelier_conversation::TurnStatus::Failed(reason) => TurnStatus::Failed(reason),
        },
    }
}

pub fn convert(event: agentelier_conversation::ConversationEvent) -> ConversationEvent {
    match event {
        agentelier_conversation::ConversationEvent::Archives(entries) => {
            ConversationEvent::Archives(
                entries
                    .into_iter()
                    .map(|entry| ArchiveEntry {
                        id: entry.summary.session,
                        title: entry.summary.title,
                        created_at_ms: entry.summary.created_at_ms,
                        last_activity_ms: entry.summary.last_activity_ms,
                        last_sequence: entry.summary.last_sequence,
                        error: entry.error,
                    })
                    .collect(),
            )
        }
        agentelier_conversation::ConversationEvent::Opened(session) => {
            ConversationEvent::Opened(view(session))
        }
        agentelier_conversation::ConversationEvent::ArchiveFailed(reason) => {
            ConversationEvent::ArchiveFailed(reason)
        }
        agentelier_conversation::ConversationEvent::Started(session) => {
            ConversationEvent::Started(view(session))
        }
        agentelier_conversation::ConversationEvent::Queue(event) => {
            ConversationEvent::Queue(QueueEvent {
                id: event.id,
                view: event.view.map(view),
                decision: match event.decision {
                    agentelier_conversation::QueueDecision::Enqueued => QueueDecision::Enqueued,
                    agentelier_conversation::QueueDecision::Removed { operation } => {
                        QueueDecision::Removed { operation }
                    }
                    agentelier_conversation::QueueDecision::AlreadyStarted { operation } => {
                        QueueDecision::AlreadyStarted { operation }
                    }
                    agentelier_conversation::QueueDecision::Resumed => QueueDecision::Resumed,
                    agentelier_conversation::QueueDecision::Rejected(reason) => {
                        QueueDecision::Rejected(reason)
                    }
                },
            })
        }
        agentelier_conversation::ConversationEvent::InputReceipt(receipt) => {
            ConversationEvent::InputReceipt(InputReceipt {
                id: receipt.id,
                decision: match receipt.decision {
                    agentelier_conversation::InputDecision::Accepted => InputDecision::Accepted,
                    agentelier_conversation::InputDecision::Applied => InputDecision::Applied,
                    agentelier_conversation::InputDecision::Rejected(reason) => {
                        InputDecision::Rejected(reason)
                    }
                },
            })
        }
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

pub fn steer_input(
    id: u64,
    session: SessionId,
    turn: u64,
    operation_id: u64,
    text: String,
) -> agentelier_conversation::SteerInput {
    agentelier_conversation::SteerInput {
        id,
        session,
        turn: agentelier_conversation::TurnId(turn),
        operation_id,
        text,
    }
}

pub fn session_command(id: u64, action: SessionAction) -> agentelier_conversation::SessionCommand {
    match action {
        SessionAction::Enqueue { session, text } => {
            agentelier_conversation::SessionCommand::Enqueue { id, session, text }
        }
        SessionAction::Remove { session, operation } => {
            agentelier_conversation::SessionCommand::Remove {
                id,
                session,
                operation,
            }
        }
        SessionAction::Resume { session } => {
            agentelier_conversation::SessionCommand::Resume { id, session }
        }
    }
}
