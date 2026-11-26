use super::ModelControl;
use agentelier_conversation::{
    AgentId, Conversation, ConversationEvent, ConversationFailure, PreparedSession, ProjectId,
    RecordEvent, SessionId, SessionSettings, SessionView, SteerInput,
};
use agentelier_session_store::{Commit, SessionStore};
use std::io;
use std::path::Path;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info};

pub(super) struct Creation {
    pub(super) id: u64,
    pub(super) control: ModelControl,
    prepared: PreparedSession,
    reply: oneshot::Receiver<io::Result<Commit>>,
    pub(super) first: Option<FirstMessage>,
}

pub(super) struct FirstMessage {
    pub(super) input: String,
    pub(super) steers: mpsc::Receiver<SteerInput>,
    pub(super) task_actions: mpsc::Receiver<agentelier_turn_runner::SessionTaskCommand>,
}

pub(super) async fn freeze_origin(
    mut settings: SessionSettings,
    data_root: &Path,
) -> Result<SessionSettings, ConversationFailure> {
    let path = data_root.join("persona").join("origin.md");
    let snapshot = settings.instructions.clone();
    settings.instructions = tokio::task::spawn_blocking(move || snapshot.with_origin(&path))
        .await
        .map_err(|failure| ConversationFailure::new(format!("Origin 人格加载任务失败：{failure}")))?
        .map_err(|failure| ConversationFailure::new(failure.to_string()))?
        .into();
    Ok(settings)
}

pub(super) async fn save_working_directory(
    id: u64,
    session: SessionId,
    directory: &Path,
    conversation: &mut Conversation,
    store: &SessionStore,
) -> Result<SessionView, ConversationFailure> {
    let directory = conversation.prepare_working_directory(session, directory)?;
    if let Some(current) = conversation.current()
        && current.settings.working_directory == directory
    {
        info!(operation_id = id, session_id = %session, "会话工作目录未变化");
        return Ok(current);
    }
    let commit = store
        .client()
        .append(RecordEvent::WorkingDirectoryChanged {
            session,
            directory: directory.clone(),
        })
        .await
        .map_err(|failure| ConversationFailure::new(format!("会话工作目录保存失败：{failure}")))?;
    if commit.session != session {
        return Err(ConversationFailure::new("工作目录保存回执的会话不一致"));
    }
    if let Some(failure) = commit.summary_failure {
        error!(session_id = %session, error = %failure, "工作目录已保存，摘要更新失败，可从记录重建");
    }
    let view = conversation.install_working_directory(session, directory)?;
    info!(operation_id = id, session_id = %session, sequence = commit.sequence, "会话工作目录已持久保存");
    Ok(view)
}

impl Creation {
    pub(super) fn start(
        id: u64,
        control: ModelControl,
        prepared: PreparedSession,
        project: Option<ProjectId>,
        store: &SessionStore,
    ) -> Result<Self, ConversationFailure> {
        let reply = store
            .create(prepared.initial.clone(), project)
            .map_err(|failure| ConversationFailure::new(format!("无法创建会话存档：{failure}")))?;
        info!(operation_id = id, session_id = %prepared.initial.session, "开始创建会话存档");
        Ok(Self {
            id,
            control,
            prepared,
            reply,
            first: None,
        })
    }

    pub(super) async fn wait(&mut self) -> io::Result<Commit> {
        (&mut self.reply)
            .await
            .map_err(|_| io::Error::other("会话存储工作者未返回提交结果"))?
    }

    pub(super) fn complete(
        self,
        result: io::Result<Commit>,
        conversation: &mut Conversation,
    ) -> (ConversationEvent, Option<FirstMessage>) {
        let commit = match result {
            Ok(commit) => commit,
            Err(failure) => {
                self.control.finished.cancel();
                return (
                    ConversationEvent::Rejected(format!("无法创建会话存档：{failure}")),
                    None,
                );
            }
        };
        if commit.session != self.prepared.initial.session {
            self.control.finished.cancel();
            return (
                ConversationEvent::Rejected("会话存档提交身份不匹配".into()),
                None,
            );
        }
        if let Some(failure) = commit.summary_failure {
            error!(session_id = %commit.session, error = %failure, "会话初始记录已保存，摘要更新失败，可从记录重建");
        }
        let agent = AgentId::for_root(commit.session);
        match conversation.install(self.prepared) {
            Ok((view, instance)) => {
                info!(operation_id = self.id, session_id = %view.id, agent_id = %agent, agent_path = "root", runtime_instance = %instance, model = %view.settings.slug, effort = ?view.settings.effort, sequence = commit.sequence, "会话初始存档已提交并登记 Root Agent");
                if self.first.is_some() {
                    (ConversationEvent::CreatedForSend(view), self.first)
                } else {
                    self.control.finished.cancel();
                    (ConversationEvent::Created(view), None)
                }
            }
            Err(failure) => {
                self.control.finished.cancel();
                error!(operation_id = self.id, session_id = %commit.session, agent_id = %agent, agent_path = "root", error = %failure, "会话已保存，Root Agent 登记失败");
                (
                    ConversationEvent::Rejected(format!(
                        "会话已保存，但无法登记 Root Agent：{failure}"
                    )),
                    None,
                )
            }
        }
    }
}
