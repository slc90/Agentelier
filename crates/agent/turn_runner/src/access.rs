use agentelier_conversation::{
    ActiveTurn, AudioAttachment, Conversation, ConversationFailure, InputReceipt, RecordEvent,
    SessionId,
};
use agentelier_tools::{ImagePayload, OriginalImage};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub enum RunnerEvent {
    Edit(TurnEdit),
    Record(Box<TurnRecord>),
    Image(TurnImage),
    GeneratedImage(TurnGeneratedImage),
    PublishedAudio(TurnPublishedAudio),
    Admitted,
    Progress,
    TaskProgress,
    Receipt(InputReceipt),
    Inbox(InboxRequest),
}

pub struct InboxRequest {
    pub thread: SessionId,
    pub turn: agentelier_conversation::TurnId,
    pub action: InboxAction,
}

pub enum InboxAction {
    Take(
        oneshot::Sender<
            Result<Option<agentelier_conversation::ClaimedMessage>, ConversationFailure>,
        >,
    ),
    Consumed {
        claim: agentelier_conversation::MessageClaim,
        reply: oneshot::Sender<Result<(), ConversationFailure>>,
    },
}

impl InboxRequest {
    pub fn close(self) {
        match self.action {
            InboxAction::Take(reply) => drop(reply.send(Ok(None))),
            InboxAction::Consumed { reply, .. } => {
                drop(reply.send(Err(ConversationFailure::new("邮箱已停止接纳消费回执"))))
            }
        }
    }
}

type Edit = dyn for<'a> FnOnce(Result<ActiveTurn<'a>, ConversationFailure>) + Send;

pub struct TurnEdit(Box<Edit>);

impl TurnEdit {
    pub fn apply(self, conversation: &mut Conversation, operation: u64) {
        let scope = if operation == 0 {
            conversation.tool_turn()
        } else {
            conversation.active_turn(operation)
        };
        self.0(scope);
    }
}

pub struct TurnRecord {
    pub event: RecordEvent,
    pub reply: oneshot::Sender<Result<u64, ConversationFailure>>,
}

pub struct TurnImage {
    pub session: SessionId,
    pub summary: String,
    pub image: Arc<ImagePayload>,
    pub reply: oneshot::Sender<Result<Arc<ImagePayload>, ConversationFailure>>,
}

pub struct TurnGeneratedImage {
    pub session: SessionId,
    pub call_id: String,
    pub image: Arc<OriginalImage>,
    pub reply: oneshot::Sender<Result<SavedGeneratedImage, ConversationFailure>>,
}

pub struct TurnPublishedAudio {
    pub session: SessionId,
    pub call_id: String,
    pub path: std::path::PathBuf,
    pub reply: oneshot::Sender<Result<AudioAttachment, ConversationFailure>>,
}

pub struct SavedGeneratedImage {
    pub path: std::path::PathBuf,
    pub feedback: GeneratedImageFeedback,
}

pub enum GeneratedImageFeedback {
    Unsupported,
    Ready(Arc<ImagePayload>),
    Unavailable(String),
}

#[derive(Clone)]
pub(crate) struct TurnAccess {
    sender: mpsc::Sender<RunnerEvent>,
}

impl TurnAccess {
    pub(crate) fn new(sender: mpsc::Sender<RunnerEvent>) -> Self {
        Self { sender }
    }

    pub(crate) async fn apply_inbox(&self) -> Result<(), ConversationFailure> {
        let scope = self
            .call(|turn| {
                Ok(turn
                    .can_apply_mailbox()
                    .then(|| (turn.session_id(), turn.turn_id())))
            })
            .await?;
        let Some((thread, turn)) = scope else {
            return Ok(());
        };
        let turn = agentelier_conversation::TurnId(turn?);
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::Inbox(InboxRequest {
                thread,
                turn,
                action: InboxAction::Take(reply),
            }))
            .await
            .map_err(|_| ConversationFailure::new("轮次邮箱接收端已关闭"))?;
        let Some(message) = result
            .await
            .map_err(|_| ConversationFailure::new("轮次邮箱未返回领取结果"))??
        else {
            return Ok(());
        };
        let claim = message.claim;
        let record = self
            .call(move |mut turn| turn.apply_mailbox(message))
            .await?;
        let sequence = self.record(record).await?;
        self.call(move |mut turn| {
            turn.confirm_history_commit(sequence);
            Ok(())
        })
        .await?;
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::Inbox(InboxRequest {
                thread,
                turn,
                action: InboxAction::Consumed { claim, reply },
            }))
            .await
            .map_err(|_| ConversationFailure::new("邮箱消费回执接收端已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("邮箱未确认消费"))?
    }

    pub(crate) async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(ActiveTurn<'_>) -> Result<T, ConversationFailure> + Send + 'static,
    ) -> Result<T, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        let edit = TurnEdit(Box::new(move |turn| {
            drop(reply.send(turn.and_then(operation)));
        }));
        self.sender
            .send(RunnerEvent::Edit(edit))
            .await
            .map_err(|_| ConversationFailure::new("会话状态所有者已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("会话状态操作未返回结果"))?
    }

    pub(crate) async fn record(&self, event: RecordEvent) -> Result<u64, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::Record(Box::new(TurnRecord { event, reply })))
            .await
            .map_err(|_| ConversationFailure::new("会话记录提交接收端已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("会话记录提交未返回结果"))?
    }

    pub(crate) async fn publish_tasks(&self) -> Result<(), ConversationFailure> {
        self.sender
            .send(RunnerEvent::TaskProgress)
            .await
            .map_err(|_| ConversationFailure::new("任务状态接收端已关闭"))
    }

    pub(crate) async fn persist_image(
        &self,
        session: SessionId,
        summary: String,
        image: Arc<ImagePayload>,
    ) -> Result<Arc<ImagePayload>, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::Image(TurnImage {
                session,
                summary,
                image,
                reply,
            }))
            .await
            .map_err(|_| ConversationFailure::new("图片附件保存接收端已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("图片附件保存未返回结果"))?
    }

    pub(crate) async fn persist_generated_image(
        &self,
        session: SessionId,
        call_id: String,
        image: Arc<OriginalImage>,
    ) -> Result<SavedGeneratedImage, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::GeneratedImage(TurnGeneratedImage {
                session,
                call_id,
                image,
                reply,
            }))
            .await
            .map_err(|_| ConversationFailure::new("原图保存接收端已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("原图保存未返回结果"))?
    }

    pub(crate) async fn persist_published_audio(
        &self,
        session: SessionId,
        call_id: String,
        path: std::path::PathBuf,
    ) -> Result<AudioAttachment, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(RunnerEvent::PublishedAudio(TurnPublishedAudio {
                session,
                call_id,
                path,
                reply,
            }))
            .await
            .map_err(|_| ConversationFailure::new("助手音频保存接收端已关闭"))?;
        result
            .await
            .map_err(|_| ConversationFailure::new("助手音频保存未返回结果"))?
    }
}
