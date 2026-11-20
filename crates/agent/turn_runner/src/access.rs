use agentelier_conversation::{
    ActiveTurn, Conversation, ConversationFailure, InputReceipt, RecordEvent, SessionId,
};
use agentelier_tools::ImagePayload;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

pub enum RunnerEvent {
    Edit(TurnEdit),
    Record(Box<TurnRecord>),
    Image(TurnImage),
    Progress,
    TaskProgress,
    Receipt(InputReceipt),
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

pub(crate) struct TurnAccess {
    sender: mpsc::Sender<RunnerEvent>,
}

impl TurnAccess {
    pub(crate) fn new(sender: mpsc::Sender<RunnerEvent>) -> Self {
        Self { sender }
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
}
