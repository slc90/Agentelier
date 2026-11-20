use agentelier_conversation::{Conversation, ConversationFailure, SessionId, TaskPolicyChange};
use agentelier_session_store::SessionStore;
use agentelier_turn_runner::{SessionTaskAction, SessionTaskOutcome};

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
