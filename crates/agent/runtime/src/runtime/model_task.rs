use super::{AgentEvent, AgentEventKind, ModelControl, RuntimeFailure, emit};
use agentelier_account::{CredentialClient, DeliveryFailure};
use agentelier_conversation::{
    ActiveTurn, Conversation, ConversationEvent, ConversationFailure, SessionView,
};
use agentelier_llm_model::{ModelFailure, ModelFailureKind, ModelRequest, ModelResult};
use tokio::sync::mpsc;
use tracing::debug;

pub(super) const PROGRESS_CAPACITY: usize = 32;
pub(super) type ModelJoin = Result<Option<TaskResult>, tokio::task::JoinError>;
pub(super) enum TaskResult {
    Model(Result<ModelResult, ModelFailure>),
    Chat(Result<Box<ActiveTurn>, ConversationFailure>),
}
pub(super) enum TaskRequest {
    Model(ModelRequest),
    Chat(Box<ActiveTurn>, mpsc::Sender<SessionView>),
}
pub(super) struct ModelTask {
    pub(super) id: u64,
    pub(super) chat: bool,
    pub(super) control: ModelControl,
    pub(super) handle: tokio::task::JoinHandle<Option<TaskResult>>,
}
impl ModelTask {
    pub(super) fn start(
        id: u64,
        control: ModelControl,
        credentials: CredentialClient,
        request: TaskRequest,
    ) -> Self {
        let cancel = control.cancel.clone();
        let chat = matches!(&request, TaskRequest::Chat(..));
        let handle = tokio::spawn(async move {
            debug!(operation_id = id, chat, "模型操作执行体已启动");
            let result = match request {
                TaskRequest::Model(request) => tokio::select! { biased;
                    result = agentelier_llm_model::execute(credentials, request) => Some(TaskResult::Model(result)),
                    _ = cancel.cancelled() => None,
                },
                TaskRequest::Chat(active, progress) => Some(TaskResult::Chat(
                    agentelier_turn_runner::run(*active, credentials, cancel, progress)
                        .await
                        .map(Box::new),
                )),
            };
            debug!(operation_id = id, chat, "模型操作执行体已结束");
            result
        });
        Self {
            id,
            chat,
            control,
            handle,
        }
    }
}

pub(super) fn progress(
    conversation: &mut Conversation,
    id: u64,
    event: SessionView,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    let view = conversation.update(id, event)?;
    emit(
        publish,
        id,
        AgentEventKind::Conversation(ConversationEvent::Updated(view)),
    )
}

pub(super) fn drain_progress(
    conversation: &mut Conversation,
    id: u64,
    incoming: &mut Option<mpsc::Receiver<SessionView>>,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(receiver) = incoming {
        while let Ok(event) = receiver.try_recv() {
            progress(conversation, id, event, publish)?;
        }
    }
    *incoming = None;
    Ok(())
}

fn task_failure() -> ModelFailure {
    ModelFailure {
        kind: ModelFailureKind::Service,
        status: None,
        code: None,
        request_id: None,
        message: "模型操作任务异常结束".into(),
    }
}

pub(super) fn complete(
    conversation: &mut Conversation,
    id: u64,
    chat: bool,
    result: ModelJoin,
) -> Result<AgentEventKind, RuntimeFailure> {
    if chat {
        let view = match result {
            Ok(Some(TaskResult::Chat(Ok(active)))) => conversation.finish(id, *active)?,
            Ok(Some(TaskResult::Chat(Err(failure)))) => {
                tracing::error!(operation_id = id, error = %failure, "聊天执行记录无法归还");
                conversation.interrupt(id)?
            }
            Err(_) => conversation.interrupt(id)?,
            _ => return Err(RuntimeFailure::ModelState),
        };
        Ok(AgentEventKind::Conversation(ConversationEvent::Finished(
            view,
        )))
    } else {
        match result {
            Ok(Some(TaskResult::Model(result))) => Ok(AgentEventKind::ModelCompleted(result)),
            Ok(None) => Ok(AgentEventKind::ModelCancelled),
            Err(_) => Ok(AgentEventKind::ModelCompleted(Err(task_failure()))),
            _ => Err(RuntimeFailure::ModelState),
        }
    }
}

pub(super) async fn stop_model(
    model: &mut Option<ModelTask>,
    incoming: &mut Option<mpsc::Receiver<SessionView>>,
    conversation: &mut Conversation,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(task) = model.take() {
        task.control.cancel.cancel();
        debug!(operation_id = task.id, "已请求取消模型操作，等待执行体结束");
        let result = task.handle.await;
        task.control.finished.cancel();
        drain_progress(conversation, task.id, incoming, publish)?;
        debug!(operation_id = task.id, "模型操作已真实结束并回收");
        emit(
            publish,
            task.id,
            complete(conversation, task.id, task.chat, result)?,
        )?;
    }
    Ok(())
}
