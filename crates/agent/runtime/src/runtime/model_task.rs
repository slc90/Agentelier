use super::{AgentEvent, AgentEventKind, ModelControl, RuntimeFailure, SharedModel, emit};
use agentelier_account::{CredentialClient, DeliveryFailure};
use agentelier_conversation::{Conversation, ConversationEvent, ConversationFailure, SteerInput};
use agentelier_llm_model::{ModelFailure, ModelFailureKind, ModelRequest, ModelResult};
use agentelier_turn_runner::RunnerEvent;
use tokio::sync::mpsc;
use tracing::debug;

pub(super) const PROGRESS_CAPACITY: usize = 32;
pub(super) type ModelJoin = Result<Option<TaskResult>, tokio::task::JoinError>;
pub(super) enum TaskResult {
    Model(Result<ModelResult, ModelFailure>),
    Chat(Result<(), ConversationFailure>),
}
pub(super) enum TaskRequest {
    Model(ModelRequest),
    Chat(
        mpsc::Sender<RunnerEvent>,
        mpsc::Receiver<SteerInput>,
        mpsc::Receiver<agentelier_turn_runner::SessionTaskCommand>,
        std::path::PathBuf,
        Option<agentelier_memory::MemoryClient>,
        super::tool_jobs::SessionTools,
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
                TaskRequest::Chat(
                    progress,
                    steers,
                    task_actions,
                    user_skills_root,
                    memory,
                    tools,
                ) => {
                    let outcome = agentelier_turn_runner::run(
                        credentials,
                        cancel.clone(),
                        progress,
                        steers,
                        user_skills_root,
                        memory,
                        tools.dispatcher.clone(),
                        tools.work.clone(),
                        task_actions,
                        tools.model_connection.clone(),
                    )
                    .await;
                    Some(TaskResult::Chat(outcome))
                }
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
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    match event {
        RunnerEvent::Edit(edit) => {
            edit.apply(conversation, id);
            Ok(())
        }
        RunnerEvent::Record(record) => {
            let agentelier_turn_runner::TurnRecord { event, reply } = *record;
            let session = event.session();
            let target = if id == 0 {
                conversation.tool_turn()
            } else {
                conversation.active_turn(id)
            };
            if let Err(failure) = target.and_then(|turn| turn.validate_record(&event)) {
                drop(reply.send(Err(failure)));
                return Ok(());
            }
            let store = store.client();
            tokio::spawn(async move {
                let result = store
                    .append(event)
                    .await
                    .map_err(|failure| {
                        ConversationFailure::new(format!("会话记录保存失败：{failure}"))
                    })
                    .and_then(|commit| {
                        if commit.session == session {
                            Ok(commit.sequence)
                        } else {
                            Err(ConversationFailure::new("会话记录提交身份不一致"))
                        }
                    });
                drop(reply.send(result));
            });
            Ok(())
        }
        RunnerEvent::Image(attachment) => {
            let agentelier_turn_runner::TurnImage {
                session,
                summary,
                image,
                reply,
            } = attachment;
            let target = if id == 0 {
                conversation.tool_turn()
            } else {
                conversation.active_turn(id)
            };
            let valid = target.is_ok_and(|turn| turn.session_id() == session);
            if !valid {
                drop(reply.send(Err(ConversationFailure::new("图片附件不属于当前任务"))));
                return Ok(());
            }
            let store = store.client();
            tokio::spawn(async move {
                let result = match store.save_image(session, summary, image).await {
                    Ok(reference) => store.read_image(reference).await,
                    Err(failure) => Err(failure),
                };
                drop(
                    reply.send(
                        result.map_err(|failure| ConversationFailure::new(failure.to_string())),
                    ),
                );
            });
            Ok(())
        }
        RunnerEvent::Progress => {
            if id == 0 {
                conversation.tool_turn()?;
            } else {
                conversation.active_turn(id)?;
            }
            let view = conversation.current().ok_or(RuntimeFailure::ModelState)?;
            emit(
                publish,
                id,
                AgentEventKind::Conversation(if id == 0 {
                    ConversationEvent::IdleUpdated(view)
                } else {
                    ConversationEvent::Updated(view)
                }),
            )
        }
        RunnerEvent::TaskProgress => {
            if id == 0 {
                conversation.tool_turn()?;
            } else {
                conversation.active_turn(id)?;
            }
            publish_tasks(conversation, publish)
        }
        RunnerEvent::Receipt(receipt) => emit(
            publish,
            receipt.id,
            AgentEventKind::Conversation(ConversationEvent::InputReceipt(receipt)),
        ),
    }
}

pub(super) fn publish_tasks(
    conversation: &Conversation,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(tasks) = conversation.tasks() {
        debug!(session = %tasks.session, revision = tasks.revision, count = tasks.tasks.len(), "已发布会话任务状态");
        emit(
            publish,
            0,
            AgentEventKind::Conversation(ConversationEvent::Tasks(tasks)),
        )?;
    }
    Ok(())
}

pub(super) fn drain_progress(
    conversation: &mut Conversation,
    id: u64,
    incoming: &mut Option<mpsc::Receiver<RunnerEvent>>,
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    if let Some(receiver) = incoming {
        while let Ok(event) = receiver.try_recv() {
            progress(conversation, id, event, store, publish)?;
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
            Ok(Some(TaskResult::Chat(Ok(())))) => conversation.finish(id)?,
            Ok(Some(TaskResult::Chat(Err(failure)))) => {
                tracing::error!(operation_id = id, error = %failure, "聊天执行记录未确认");
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

pub(super) async fn drive_idle(
    conversation: &mut Conversation,
    tools: &super::tool_jobs::SessionTools,
    action: agentelier_turn_runner::IdleAction,
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    let (outgoing, mut incoming) = mpsc::channel(PROGRESS_CAPACITY);
    let operation = agentelier_turn_runner::run_idle(
        tools.work.clone(),
        tools.dispatcher.clone(),
        action,
        outgoing,
    );
    tokio::pin!(operation);
    let result = loop {
        tokio::select! { biased;
            Some(event) = incoming.recv() => progress(conversation, 0, event, store, publish)?,
            result = &mut operation => break result,
        }
    };
    while let Ok(event) = incoming.try_recv() {
        progress(conversation, 0, event, store, publish)?;
    }
    result.map_err(Into::into)
}

pub(super) async fn finish_tools(
    tools: super::tool_jobs::SessionTools,
    reason: &'static str,
    conversation: &mut Conversation,
    store: &agentelier_session_store::SessionStore,
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<(), RuntimeFailure> {
    tools.work.lock().await.stop_all();
    let mut failure = None;
    while tools.busy() {
        let event = { tools.work.lock().await.next().await };
        let Some(event) = event else {
            failure = Some(RuntimeFailure::ModelState);
            break;
        };
        if let Err(error) = drive_idle(
            conversation,
            &tools,
            agentelier_turn_runner::IdleAction::Tool(event),
            store,
            publish,
        )
        .await
        {
            tracing::error!(%error, "会话工具终态收尾失败");
            failure.get_or_insert(error);
        }
    }
    let report = tools.join(reason).await?;
    if let Some(error) = failure {
        return Err(error);
    }
    if report.failed {
        return Err(ConversationFailure::new("会话资源回收或保存未确认").into());
    }
    Ok(())
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
                    Some(event) => if let Err(failure) = progress(conversation,task.id,event,store,publish) {
                        tracing::error!(error=%failure,"取消收尾时状态交付失败，继续回收任务");
                        progress_failure=progress_failure.or(Some(failure));
                    },
                    None => *incoming = None,
                },
            }
        };
        task.control.finished.cancel();
        if let Err(failure) = drain_progress(conversation, task.id, incoming, store, publish) {
            tracing::error!(error=%failure,"取消收尾时末次进度交付失败");
            progress_failure = progress_failure.or(Some(failure));
        }
        debug!(operation_id = task.id, "模型操作已真实结束并回收");
        let mut event = complete(conversation, task.id, task.chat, result)?;
        let unconfirmed = match &event {
            AgentEventKind::Conversation(ConversationEvent::Finished(view))
                if view.execution_blocked =>
            {
                view.turn.as_ref().and_then(|turn| match &turn.status {
                    agentelier_conversation::TurnStatus::Failed(reason) => {
                        Some(ConversationFailure::new(reason.clone()))
                    }
                    _ => None,
                })
            }
            _ => None,
        };
        let mut storage_failure = None;
        if task.chat {
            let (state, history) = conversation.archive().ok_or(RuntimeFailure::ModelState)?;
            match store
                .client()
                .append(agentelier_conversation::RecordEvent::Finished { state, history })
                .await
            {
                Ok(commit) => {
                    if conversation.settle_tasks(commit.sequence) {
                        publish_tasks(conversation, publish)?;
                    }
                }
                Err(failure) => {
                    if let Some(view) =
                        conversation.fail_storage(format!("任务已回收，但终态保存失败：{failure}"))
                    {
                        event = AgentEventKind::Conversation(ConversationEvent::Finished(view));
                    }
                    storage_failure = Some(RuntimeFailure::Store(failure));
                }
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
        if let Some(failure) = unconfirmed {
            return Err(failure.into());
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
    publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    user_skills_root: &std::path::Path,
    memory: Option<agentelier_memory::MemoryClient>,
    session_tools: &mut Option<(
        agentelier_conversation::SessionId,
        super::tool_jobs::SessionTools,
    )>,
    store: &agentelier_session_store::SessionStore,
) -> Result<(), RuntimeFailure> {
    if model.is_some() {
        return Ok(());
    }
    let Some((id, view)) = conversation.start_queued()? else {
        return Ok(());
    };
    let (control, steers, task_actions) = ModelControl::new(id);
    let tools = super::tool_jobs::SessionTools::for_session(
        session_tools,
        &view,
        conversation,
        store.client(),
    );
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
        TaskRequest::Chat(
            sender,
            steers,
            task_actions,
            user_skills_root.to_owned(),
            memory,
            tools,
        ),
    ));
    Ok(())
}
