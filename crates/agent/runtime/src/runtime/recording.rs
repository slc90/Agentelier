use super::{AgentEventKind, ModelControl, RuntimeFailure, emit};
use agentelier_account::DeliveryFailure;
use agentelier_conversation::{
    Conversation, ConversationEvent, PreparedQueue, QueueDecision, QueueEvent, RecordEvent,
    SessionId,
};
use agentelier_session_store::{Commit, SessionStore};
use std::io;
use tokio::task::JoinSet;

pub(super) enum Pending {
    Queue(PreparedQueue),
    Finished {
        id: u64,
        session: SessionId,
        control: ModelControl,
    },
}

pub(super) type Records = JoinSet<(Pending, io::Result<Commit>)>;

pub(super) struct Applied {
    pub(super) queue: bool,
    pub(super) failed: bool,
}

pub(super) fn submit(
    pending: Pending,
    event: RecordEvent,
    store: &SessionStore,
    records: &mut Records,
) {
    let reply = store.client().try_append(event);
    records.spawn(async move {
        let result = match reply {
            Ok(reply) => reply
                .await
                .unwrap_or_else(|_| Err(io::Error::other("会话记录提交未返回结果"))),
            Err(failure) => Err(failure),
        };
        (pending, result)
    });
}

pub(super) fn apply(
    pending: Pending,
    result: io::Result<Commit>,
    conversation: &mut Conversation,
    publish: &impl Fn(super::AgentEvent) -> Result<(), DeliveryFailure>,
) -> Result<Applied, RuntimeFailure> {
    let expected = match &pending {
        Pending::Queue(queue) => queue.session(),
        Pending::Finished { session, .. } => *session,
    };
    let result = result.and_then(|commit| {
        if commit.session == expected {
            Ok(commit)
        } else {
            Err(io::Error::other("会话记录提交身份不一致"))
        }
    });
    let failed = result.is_err();
    match pending {
        Pending::Queue(queue) => {
            let event = match result {
                Ok(_) => queue.commit(conversation),
                Err(failure) => {
                    let message = format!("待办记录保存失败：{failure}");
                    if conversation
                        .current()
                        .is_some_and(|view| view.id == expected)
                    {
                        conversation.fail_storage(message.clone());
                    }
                    QueueEvent {
                        id: queue.id,
                        view: conversation.current(),
                        decision: QueueDecision::Rejected(message),
                    }
                }
            };
            emit(
                publish,
                event.id,
                AgentEventKind::Conversation(ConversationEvent::Queue(event)),
            )?;
            Ok(Applied {
                queue: true,
                failed,
            })
        }
        Pending::Finished { id, control, .. } => {
            control.finished.cancel();
            let view = if conversation
                .current()
                .is_some_and(|view| view.id == expected)
            {
                match result {
                    Ok(_) => conversation.current(),
                    Err(failure) => conversation
                        .fail_storage(format!("任务已真实结束，但终态保存失败：{failure}")),
                }
            } else {
                None
            };
            let event = match view {
                Some(view) => ConversationEvent::Finished(view),
                None => ConversationEvent::Rejected("任务已回收，当前会话已变化".into()),
            };
            emit(publish, id, AgentEventKind::Conversation(event))?;
            Ok(Applied {
                queue: false,
                failed,
            })
        }
    }
}
