use agentelier_conversation::{
    Conversation, ConversationFailure, SessionId, TaskAction, TaskActionOutcome, TaskActionReceipt,
    TaskCancelState, TaskPolicyChange,
};
use agentelier_session_store::SessionStore;
use agentelier_turn_runner::{SessionTaskAction, SessionTaskOutcome};
use tokio::sync::oneshot;
use tokio::task::JoinSet;

pub(super) type Receipts = JoinSet<(u64, TaskActionReceipt)>;

pub(super) fn reject(receipts: &mut Receipts, id: u64, session: SessionId, reason: &str) {
    let reason = reason.to_owned();
    receipts.spawn(async move {
        (
            id,
            TaskActionReceipt {
                session,
                result: Err(reason),
            },
        )
    });
}

pub(super) async fn dispatch(
    receipts: &mut Receipts,
    id: u64,
    session: SessionId,
    action: TaskAction,
    conversation: &mut Conversation,
    model: Option<&super::model_task::ModelTask>,
    tools: Option<&super::tool_jobs::SessionTools>,
    store: &SessionStore,
) {
    if conversation
        .current()
        .is_none_or(|current| current.id != session)
    {
        return reject(receipts, id, session, "任务控制会话已变化");
    }
    let action = match action {
        TaskAction::Terminate { .. }
        | TaskAction::ReadExit { .. }
        | TaskAction::DiscardExit { .. } => {
            let Some(processes) = tools.map(|tools| tools.processes.clone()) else {
                return reject(receipts, id, session, "该历史进程无本次运行句柄");
            };
            receipts.spawn(async move {
                let result = match action {
                    TaskAction::Terminate { process, instance } => processes
                        .terminate(&process, instance)
                        .await
                        .map(|()| TaskActionOutcome::Status("进程已退出并回收".into())),
                    TaskAction::ReadExit { process, instance } => processes
                        .read_exit(&process, instance, false)
                        .await
                        .and_then(|output| {
                            output
                                .map(TaskActionOutcome::ProcessOutput)
                                .ok_or_else(|| "进程退出结果缺少可读内容".into())
                        }),
                    TaskAction::DiscardExit { process, instance } => processes
                        .read_exit(&process, instance, true)
                        .await
                        .map(|_| TaskActionOutcome::Status("已收起退出结果".into())),
                    _ => Err("无效的进程控制请求".into()),
                };
                (id, TaskActionReceipt { session, result })
            });
            return;
        }
        TaskAction::Cancel { task, instance } => {
            if !conversation.matches_task_execution(session, task, instance) {
                return reject(receipts, id, session, "任务或执行实例已变化");
            }
            SessionTaskAction::Cancel(agentelier_tools::TaskTargets::Ids(vec![task.0]))
        }
        TaskAction::StopAll => SessionTaskAction::StopAll,
        TaskAction::Resume => SessionTaskAction::Resume,
        TaskAction::ConfirmExternal { call_ids } => SessionTaskAction::ConfirmExternal { call_ids },
    };
    if let Some(model) = model {
        if model
            .control
            .turn
            .get()
            .is_none_or(|(current, _)| *current != session)
        {
            return reject(receipts, id, session, "当前运行不属于目标会话");
        }
        let (reply, response) = oneshot::channel();
        let control = agentelier_turn_runner::SessionTaskCommand { action, reply };
        if model.control.task_actions.try_send(control).is_err() {
            return reject(receipts, id, session, "当前任务正在收尾或控制通道已满");
        }
        receipts.spawn(async move {
            let result = response
                .await
                .map_err(|_| "任务控制未返回结果".to_owned())
                .and_then(|result| result.map_err(|error| error.to_string()))
                .map(outcome);
            (id, TaskActionReceipt { session, result })
        });
    } else {
        let result = idle(conversation, session, action, store)
            .await
            .map(outcome)
            .map_err(|error| error.to_string());
        if let Some(tools) = tools
            && let Some(view) = conversation.current()
        {
            tools.admission.pause(view.task_policy.background_paused);
            tools
                .admission
                .external_unconfirmed(!view.task_policy.external_unconfirmed.is_empty());
        }
        receipts.spawn(async move { (id, TaskActionReceipt { session, result }) });
    }
}

fn outcome(outcome: SessionTaskOutcome) -> TaskActionOutcome {
    let status = match outcome {
        SessionTaskOutcome::Cancellation(receipts) => {
            match receipts.first().map(|receipt| &receipt.state) {
                Some(TaskCancelState::Requested) => "已请求取消，正在等待实际终态",
                Some(TaskCancelState::AlreadyRequested) => "已请求取消，正在回收",
                Some(TaskCancelState::Finished) | None => "任务已结束",
            }
        }
        SessionTaskOutcome::StopRequested => "已暂停后台派发和自动待办，正在回收已接纳工作",
        SessionTaskOutcome::Resumed => "后台派发已恢复",
        SessionTaskOutcome::ExternalConfirmed => "已保存远端效果确认",
    };
    TaskActionOutcome::Status(status.into())
}

pub(super) async fn idle(
    conversation: &mut Conversation,
    session: SessionId,
    action: SessionTaskAction,
    store: &SessionStore,
) -> Result<SessionTaskOutcome, ConversationFailure> {
    let (change, outcome) = match action {
        SessionTaskAction::Cancel(targets) => {
            return conversation
                .idle_cancellation(session, &targets)
                .map(SessionTaskOutcome::Cancellation)
                .ok_or_else(|| ConversationFailure::new("目标任务不存在或所属会话已变化"));
        }
        SessionTaskAction::StopAll => (TaskPolicyChange::Pause, SessionTaskOutcome::StopRequested),
        SessionTaskAction::Resume => (TaskPolicyChange::Resume, SessionTaskOutcome::Resumed),
        SessionTaskAction::ConfirmExternal { call_ids } => (
            TaskPolicyChange::ConfirmExternal { call_ids },
            SessionTaskOutcome::ExternalConfirmed,
        ),
    };
    let event = conversation.task_policy_record(session, change.clone())?;
    match store.client().append(event).await {
        Ok(commit) if commit.session == session => {}
        result => {
            let failure = result.err().map_or_else(
                || "任务控制提交会话身份不一致".into(),
                |error| format!("任务控制保存失败：{error}"),
            );
            conversation.fail_storage(failure.clone());
            return Err(ConversationFailure::new(failure));
        }
    }
    conversation.apply_task_policy(session, &change)?;
    Ok(outcome)
}
