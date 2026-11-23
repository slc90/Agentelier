use super::model_task::{self, drive_idle};
use super::tool_jobs::SessionTools;
use super::{AgentEvent, AgentEventKind, DeliveryFailure, RuntimeFailure, emit};
use agentelier_conversation::{Conversation, ConversationEvent, SessionId};
use agentelier_session_store::SessionStoreClient;
use agentelier_turn_runner::{IdleAction, ToolEvent};
use futures_util::future::select_all;
use std::collections::HashMap;

pub(super) struct Resident {
    pub(super) conversation: Conversation,
    pub(super) tools: SessionTools,
}

#[derive(Default)]
pub(super) struct BackgroundSessions(HashMap<SessionId, Resident>);

pub(super) enum Signal {
    Tool(Option<ToolEvent>),
    ProcessesChanged,
}

impl BackgroundSessions {
    pub(super) fn contains(&self, session: SessionId) -> bool {
        self.0.contains_key(&session)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn take(&mut self, session: SessionId) -> Option<Resident> {
        self.0.remove(&session)
    }

    pub(super) async fn detach(
        &mut self,
        conversation: Conversation,
        session_tools: Option<(SessionId, SessionTools)>,
    ) -> Result<(), RuntimeFailure> {
        let Some((session, tools)) = session_tools else {
            return Ok(());
        };
        if conversation.session_id() != Some(session) || self.0.contains_key(&session) {
            return Err(RuntimeFailure::ModelState);
        }
        if tools.retained_work() {
            tracing::debug!(%session, "保留非前台会话的后台工具与进程资源");
            self.0.insert(
                session,
                Resident {
                    conversation,
                    tools,
                },
            );
        } else {
            tools.join("session_replaced").await?;
        }
        Ok(())
    }

    pub(super) async fn next(&mut self) -> (SessionId, Signal) {
        let events = self.0.iter_mut().map(|(&session, resident)| {
            Box::pin(async move {
                let signal = tokio::select! {
                    biased;
                    result = resident.tools.process_views.changed() => match result {
                        Ok(()) => Signal::ProcessesChanged,
                        Err(_) => Signal::Tool(None),
                    },
                    event = async { resident.tools.work.lock().await.next().await } => Signal::Tool(event),
                };
                (session, signal)
            })
        }).collect::<Vec<_>>();
        if events.is_empty() {
            return std::future::pending().await;
        }
        select_all(events).await.0
    }

    pub(super) async fn handle(
        &mut self,
        session: SessionId,
        signal: Signal,
        store: &SessionStoreClient,
        publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        let resident = self.0.get_mut(&session).ok_or(RuntimeFailure::ModelState)?;
        match signal {
            Signal::Tool(Some(event)) => {
                if let Err(failure) = drive_idle(
                    &mut resident.conversation,
                    &resident.tools,
                    IdleAction::Tool(event),
                    store,
                    publish,
                )
                .await
                {
                    resident.tools.admission.block();
                    resident.tools.work.lock().await.stop_all();
                    tracing::error!(%session, %failure, "非前台会话的后台工具结果保存失败");
                    if let Some(view) = resident.conversation.fail_storage(failure.to_string()) {
                        emit(
                            publish,
                            0,
                            AgentEventKind::Conversation(ConversationEvent::IdleUpdated(view)),
                        )?;
                    }
                }
            }
            Signal::Tool(None) => return Err(RuntimeFailure::ModelState),
            Signal::ProcessesChanged => {
                let processes = resident.tools.process_views.borrow().projection(session);
                emit(
                    publish,
                    0,
                    AgentEventKind::Conversation(ConversationEvent::Processes(processes)),
                )?;
            }
        }
        if !resident.tools.retained_work()
            && let Some(resident) = self.0.remove(&session)
        {
            model_task::publish_tasks(&resident.conversation, publish)?;
            resident.tools.join("background_session_idle").await?;
        }
        Ok(())
    }

    pub(super) async fn finish(
        &mut self,
        reason: &'static str,
        store: &SessionStoreClient,
        publish: &impl Fn(AgentEvent) -> Result<(), DeliveryFailure>,
    ) -> Result<(), RuntimeFailure> {
        for resident in self.0.values_mut() {
            resident.tools.work.lock().await.stop_all();
        }
        let mut failure = None;
        for (session, mut resident) in self.0.drain() {
            let processes = resident.tools.process_views.clone();
            let result = model_task::finish_tools(
                resident.tools,
                reason,
                &mut resident.conversation,
                store,
                publish,
            )
            .await;
            if let Err(error) = result {
                tracing::error!(%error, "非前台会话资源收尾失败");
                failure.get_or_insert(error);
            }
            let update = processes.borrow().projection(session);
            if let Err(error) = emit(
                publish,
                0,
                AgentEventKind::Conversation(ConversationEvent::Processes(update)),
            ) {
                failure.get_or_insert(error);
            }
            if reason == "account_changed" {
                let saved = async {
                    if let Some(record) = resident.conversation.suspend_queue()? {
                        let commit = store.append(record).await.map_err(RuntimeFailure::Store)?;
                        if let Some(error) = commit.summary_failure {
                            tracing::error!(session_id = %commit.session, %error, "待办暂停已保存，摘要更新失败");
                        }
                    }
                    Ok::<(), RuntimeFailure>(())
                }.await;
                if let Err(error) = saved {
                    tracing::error!(%error, "非前台会话待办暂停未保存");
                    failure.get_or_insert(error);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}
