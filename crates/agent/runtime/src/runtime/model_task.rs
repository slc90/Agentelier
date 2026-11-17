use super::{AgentEvent, AgentEventKind, ModelControl, RuntimeFailure, SharedModel, emit};
use agentelier_account::{CredentialClient, DeliveryFailure};
use agentelier_conversation::{
    ActiveTurn, Conversation, ConversationEvent, ConversationFailure, RunnerEvent, SteerInput,
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
    Chat(
        Box<ActiveTurn>,
        mpsc::Sender<RunnerEvent>,
        mpsc::Receiver<SteerInput>,
    ),
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
        store: agentelier_session_store::SessionStoreClient,
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
                TaskRequest::Chat(active, progress, steers) => Some(TaskResult::Chat(
                    agentelier_turn_runner::run(
                        *active,
                        credentials,
                        cancel,
                        progress,
                        steers,
                        store,
                    )
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
    event: RunnerEvent,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    match event {
        RunnerEvent::Progress(view) => {
            let view = conversation.update(id, *view)?;
            emit(
                publish,
                id,
                AgentEventKind::Conversation(ConversationEvent::Updated(view)),
            )
        }
        RunnerEvent::Receipt(receipt) => emit(
            publish,
            receipt.id,
            AgentEventKind::Conversation(ConversationEvent::InputReceipt(receipt)),
        ),
    }
}

pub(super) fn drain_progress(
    conversation: &mut Conversation,
    id: u64,
    incoming: &mut Option<mpsc::Receiver<RunnerEvent>>,
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
        usage: None,
        response_id: None,
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
    incoming: &mut Option<mpsc::Receiver<RunnerEvent>>,
    conversation: &mut Conversation,
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(task) = model.take() {
        task.control.cancel.cancel();
        debug!(operation_id = task.id, "已请求取消模型操作，等待执行体结束");
        let mut progress_failure = None;
        let mut handle = task.handle;
        let result = loop {
            tokio::select! { biased;
                result = &mut handle => break result,
                event = async {match incoming.as_mut() {
                    Some(receiver) => receiver.recv().await,
                    None => std::future::pending().await,
                }} => match event {
                    Some(event) => if let Err(failure) = progress(conversation,task.id,event,publish) {
                        tracing::error!(error=%failure,"取消收尾时状态交付失败，继续回收任务");
                        progress_failure=progress_failure.or(Some(failure));
                    },
                    None => *incoming = None,
                },
            }
        };
        task.control.finished.cancel();
        if let Err(failure) = drain_progress(conversation, task.id, incoming, publish) {
            tracing::error!(error=%failure,"取消收尾时末次进度交付失败");
            progress_failure = progress_failure.or(Some(failure));
        }
        debug!(operation_id = task.id, "模型操作已真实结束并回收");
        let mut event = complete(conversation, task.id, task.chat, result)?;
        let mut storage_failure = None;
        if task.chat {
            let (state, history) = conversation.archive().ok_or(RuntimeFailure::ModelState)?;
            if let Err(failure) = store
                .client()
                .append(agentelier_conversation::RecordEvent::Finished { state, history })
                .await
            {
                if let Some(view) =
                    conversation.fail_storage(format!("任务已回收，但终态保存失败：{failure}"))
                {
                    event = AgentEventKind::Conversation(ConversationEvent::Finished(view));
                }
                storage_failure = Some(RuntimeFailure::Store(failure));
            }
        }
        let delivery = emit(publish, task.id, event);
        if let Some(failure) = progress_failure {
            return Err(failure);
        }
        delivery?;
        if let Some(failure) = storage_failure {
            return Err(failure);
        }
    }
    Ok(())
}

pub(super) fn start_queued(
    conversation: &mut Conversation,
    model: &mut Option<ModelTask>,
    incoming: &mut Option<mpsc::Receiver<RunnerEvent>>,
    shared: &SharedModel,
    credentials: &CredentialClient,
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if model.is_some() {
        return Ok(());
    }
    let Some((id, view, active)) = conversation.start_queued()? else {
        return Ok(());
    };
    let (control, steers) = ModelControl::new(id);
    let turn = view.turn.as_ref().ok_or(RuntimeFailure::ModelState)?;
    control
        .turn
        .set((view.id, turn.id.0))
        .map_err(|_| RuntimeFailure::ModelState)?;
    *shared.lock().map_err(|_| RuntimeFailure::ModelState)? = Some(control.clone());
    emit(
        publish,
        id,
        AgentEventKind::Conversation(ConversationEvent::Started(view)),
    )?;
    let (sender, receiver) = mpsc::channel(PROGRESS_CAPACITY);
    *incoming = Some(receiver);
    *model = Some(ModelTask::start(
        id,
        control,
        credentials.clone(),
        store.client(),
        TaskRequest::Chat(Box::new(active), sender, steers),
    ));
    Ok(())
}
