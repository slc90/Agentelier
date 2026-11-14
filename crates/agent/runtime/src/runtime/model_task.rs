use super::{AgentEvent, AgentEventKind, ModelControl, RuntimeFailure, emit};
use agentelier_account::{CredentialClient, DeliveryFailure};
use agentelier_conversation::{Conversation, ConversationEvent};
use agentelier_llm_model::{
    GenerationRequest, GenerationResult, ModelFailure, ModelFailureKind, ModelRequest, ModelResult,
    StreamEvent,
};
use tokio::sync::mpsc;
use tracing::debug;

pub(super) const PROGRESS_CAPACITY: usize = 32;
pub(super) type ModelJoin = Result<Option<TaskResult>, tokio::task::JoinError>;
pub(super) enum TaskResult {
    Model(Result<ModelResult, ModelFailure>),
    Chat(Result<GenerationResult, ModelFailure>),
}
pub(super) enum TaskRequest {
    Model(ModelRequest),
    Chat(GenerationRequest, mpsc::Sender<StreamEvent>),
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
            let operation = async move {
                match request {
                    TaskRequest::Model(request) => {
                        TaskResult::Model(agentelier_llm_model::execute(credentials, request).await)
                    }
                    TaskRequest::Chat(request, progress) => TaskResult::Chat(
                        agentelier_llm_model::generate(credentials, request, progress).await,
                    ),
                }
            };
            let result = tokio::select! { biased;
                _ = cancel.cancelled() => None,
                result = operation => Some(result),
            };
            debug!(
                operation_id = id,
                chat,
                cancelled = result.is_none(),
                "模型操作执行体已结束"
            );
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
    event: StreamEvent,
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
    incoming: &mut Option<mpsc::Receiver<StreamEvent>>,
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
        let outcome = match result {
            Ok(Some(TaskResult::Chat(result))) => Some(result),
            Ok(None) => None,
            Err(_) => Some(Err(task_failure())),
            _ => return Err(RuntimeFailure::ModelState),
        };
        Ok(AgentEventKind::Conversation(ConversationEvent::Finished(
            conversation.finish(id, outcome)?,
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
    incoming: &mut Option<mpsc::Receiver<StreamEvent>>,
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
