use agentelier_conversation::{
    AudioAttachment, InitialSession, ProjectId, RecordEvent, SessionId, SessionRecord, StoredImage,
};
use agentelier_tools::ImagePayload;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
use tempfile::NamedTempFile;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error};

pub const FORMAT: &str = "agentelier.session";
pub const FORMAT_VERSION: u32 = 1;
const COMMAND_CAPACITY: usize = 16;
pub(super) const INITIAL_SEQUENCE: u64 = 1;

#[derive(Serialize, Deserialize)]
pub struct Record {
    pub format: String,
    pub version: u32,
    pub sequence: u64,
    #[serde(flatten)]
    pub kind: RecordKind,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum RecordKind {
    Created(InitialSession),
    Renamed { session: SessionId, title: String },
    Deleted { session: SessionId },
    Archived { session: SessionId },
    Event(SessionRecord),
}

pub struct Commit {
    pub session: SessionId,
    pub sequence: u64,
    pub summary_failure: Option<io::Error>,
}

pub struct ArchiveCatalog {
    pub entries: Vec<agentelier_conversation::ArchiveEntry>,
    pub cleanup_errors: Vec<String>,
    pub projects: Result<agentelier_conversation::ProjectCatalog, String>,
    pub task_history: Vec<ArchivedTasks>,
}

pub struct ArchivedTasks {
    pub tasks: agentelier_conversation::SessionTasks,
    pub processes: agentelier_conversation::SessionProcesses,
}
pub struct DeleteCommit {
    pub session: SessionId,
    pub cleanup_error: Option<io::Error>,
}

pub struct MetadataCommit {
    pub summary: agentelier_conversation::SessionSummary,
    pub summary_failure: Option<io::Error>,
}

enum Command {
    AudioDraft(
        SessionId,
        Option<crate::audio::ImportedAudio>,
        oneshot::Sender<io::Result<Option<AudioAttachment>>>,
    ),
    ReadAudio(
        SessionId,
        AudioAttachment,
        oneshot::Sender<io::Result<Vec<u8>>>,
    ),
    Delete(SessionId, oneshot::Sender<io::Result<DeleteCommit>>),
    ChangeMetadata(
        SessionId,
        agentelier_conversation::SessionMetadataChange,
        oneshot::Sender<io::Result<MetadataCommit>>,
    ),
    GeneratedTitle(
        SessionId,
        String,
        oneshot::Sender<io::Result<Option<Commit>>>,
    ),
    Project(
        agentelier_conversation::ProjectChange,
        oneshot::Sender<io::Result<()>>,
    ),
    ProjectDirectory(ProjectId, oneshot::Sender<io::Result<PathBuf>>),
    Scan {
        task_history: bool,
        reply: oneshot::Sender<io::Result<ArchiveCatalog>>,
    },
    MemorySessions(oneshot::Sender<io::Result<Vec<SessionId>>>),
    MemoryCutoffs(oneshot::Sender<io::Result<Vec<crate::MemoryCutoff>>>),
    MemorySource(
        SessionId,
        oneshot::Sender<io::Result<Option<crate::MemorySourceSnapshot>>>,
    ),
    Load(
        SessionId,
        oneshot::Sender<io::Result<agentelier_conversation::RestoredSession>>,
    ),
    LoadChild(
        SessionId,
        agentelier_conversation::AgentId,
        oneshot::Sender<io::Result<agentelier_conversation::RestoredChild>>,
    ),
    Create(
        Box<InitialSession>,
        Option<ProjectId>,
        oneshot::Sender<io::Result<Commit>>,
    ),
    SaveImage(
        StoredImage,
        Arc<ImagePayload>,
        oneshot::Sender<io::Result<()>>,
    ),
    ReadImage(StoredImage, oneshot::Sender<io::Result<Arc<ImagePayload>>>),
    SaveGenerated(
        SessionId,
        Arc<agentelier_tools::OriginalImage>,
        oneshot::Sender<io::Result<PathBuf>>,
    ),
    ReadGenerated(
        SessionId,
        agentelier_tools::ImageDescriptor,
        oneshot::Sender<io::Result<agentelier_tools::OriginalImage>>,
    ),
    Append(Box<RecordEvent>, oneshot::Sender<io::Result<Commit>>),
}

#[derive(Clone)]
pub struct SessionStoreClient(mpsc::Sender<Command>, Option<AgentStorage>);

#[derive(Clone, Copy)]
struct AgentStorage {
    session: SessionId,
    id: agentelier_conversation::AgentId,
    thread: SessionId,
}

impl SessionStoreClient {
    pub fn for_agent(&self, identity: &agentelier_conversation::AgentIdentity) -> Self {
        Self(
            self.0.clone(),
            Some(AgentStorage {
                session: identity.session,
                id: identity.id,
                thread: identity.thread.session_id(),
            }),
        )
    }

    fn attachment_session(&self, thread: SessionId) -> io::Result<SessionId> {
        match self.1 {
            None => Ok(thread),
            Some(agent) if agent.thread == thread => Ok(agent.session),
            Some(_) => Err(io::Error::other("附件不属于当前 Agent Thread")),
        }
    }
    pub async fn read_audio(
        &self,
        session: SessionId,
        audio: AudioAttachment,
    ) -> io::Result<Vec<u8>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::ReadAudio(session, audio, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("音频附件读取未返回结果"))?
    }

    pub async fn change_audio_draft(
        &self,
        session: SessionId,
        path: Option<PathBuf>,
    ) -> io::Result<Option<AudioAttachment>> {
        let audio = if let Some(path) = path {
            Some(
                tokio::task::spawn_blocking(move || crate::audio::import(&path))
                    .await
                    .map_err(io::Error::other)??,
            )
        } else {
            None
        };
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::AudioDraft(session, audio, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("音频附件提交未返回结果"))?
    }

    pub fn delete(
        &self,
        session: SessionId,
    ) -> io::Result<oneshot::Receiver<io::Result<DeleteCommit>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Delete(session, reply))
            .map_err(|failure| io::Error::other(format!("无法接纳会话删除：{failure}")))?;
        Ok(result)
    }
    pub fn change_metadata(
        &self,
        session: SessionId,
        change: agentelier_conversation::SessionMetadataChange,
    ) -> io::Result<oneshot::Receiver<io::Result<MetadataCommit>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::ChangeMetadata(session, change, reply))
            .map_err(|failure| io::Error::other(format!("无法接纳会话元数据变更：{failure}")))?;
        Ok(result)
    }
    pub fn scan(&self) -> io::Result<oneshot::Receiver<io::Result<ArchiveCatalog>>> {
        self.scan_catalog(false)
    }

    pub async fn save_generated_title(
        &self,
        session: SessionId,
        title: String,
    ) -> io::Result<Option<Commit>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::GeneratedTitle(session, title, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("会话标题提交未返回结果"))?
    }

    pub fn scan_startup(&self) -> io::Result<oneshot::Receiver<io::Result<ArchiveCatalog>>> {
        self.scan_catalog(true)
    }

    fn scan_catalog(
        &self,
        task_history: bool,
    ) -> io::Result<oneshot::Receiver<io::Result<ArchiveCatalog>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Scan {
                task_history,
                reply,
            })
            .map_err(|failure| io::Error::other(format!("无法接纳会话目录读取：{failure}")))?;
        Ok(result)
    }

    pub fn change_project(
        &self,
        change: agentelier_conversation::ProjectChange,
    ) -> io::Result<oneshot::Receiver<io::Result<()>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Project(change, reply))
            .map_err(|failure| io::Error::other(format!("无法接纳项目操作：{failure}")))?;
        Ok(result)
    }

    pub async fn memory_cutoffs(&self) -> io::Result<Vec<crate::MemoryCutoff>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::MemoryCutoffs(reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("会话存储未确认记忆重置边界"))?
    }

    pub async fn memory_sessions(&self) -> io::Result<Vec<SessionId>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::MemorySessions(reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("记忆来源目录未返回读取结果"))?
    }

    pub async fn memory_source(
        &self,
        session: SessionId,
    ) -> io::Result<Option<crate::MemorySourceSnapshot>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::MemorySource(session, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("记忆来源未返回读取结果"))?
    }

    pub fn load(
        &self,
        session: SessionId,
    ) -> io::Result<oneshot::Receiver<io::Result<agentelier_conversation::RestoredSession>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Load(session, reply))
            .map_err(|failure| io::Error::other(format!("无法接纳会话读取：{failure}")))?;
        Ok(result)
    }

    pub async fn load_child(
        &self,
        session: SessionId,
        agent: agentelier_conversation::AgentId,
    ) -> io::Result<agentelier_conversation::RestoredChild> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::LoadChild(session, agent, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("子 Agent 存档读取未返回结果"))?
    }

    pub fn try_append(
        &self,
        event: RecordEvent,
    ) -> io::Result<oneshot::Receiver<io::Result<Commit>>> {
        if self.1.is_some() {
            return Err(io::Error::other("Agent Thread 记录必须等待范围校验回执"));
        }
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Append(Box::new(event), reply))
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "会话存储命令队列已满")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "会话存储工作者已关闭")
                }
            })?;
        Ok(result)
    }

    pub async fn append(&self, event: RecordEvent) -> io::Result<Commit> {
        let event = match self.1 {
            None => event,
            Some(agent) if event.session() == agent.thread => RecordEvent::AgentTurn {
                session: agent.session,
                agent: agent.id,
                event: Box::new(event),
            },
            Some(_) => return Err(io::Error::other("会话记录不属于当前 Agent Thread")),
        };
        let expected = event.session();
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::Append(Box::new(event), reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        let mut commit = result
            .await
            .map_err(|_| io::Error::other("会话记录未返回提交结果"))??;
        if let Some(agent) = self.1 {
            if commit.session != expected {
                return Err(io::Error::other("Agent 轮次写入收到所属会话不符的回执"));
            }
            commit.session = agent.thread;
        }
        Ok(commit)
    }

    pub async fn save_image(
        &self,
        session: SessionId,
        summary: String,
        image: Arc<ImagePayload>,
    ) -> io::Result<StoredImage> {
        let reference = StoredImage {
            session: self.attachment_session(session)?,
            summary,
            image: image.descriptor(),
        };
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::SaveImage(reference.clone(), image, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("附件写入未返回结果"))??;
        Ok(reference)
    }

    pub async fn read_image(&self, reference: StoredImage) -> io::Result<Arc<ImagePayload>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::ReadImage(reference, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("附件读取未返回结果"))?
    }

    pub async fn save_generated(
        &self,
        session: SessionId,
        image: Arc<agentelier_tools::OriginalImage>,
    ) -> io::Result<PathBuf> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::SaveGenerated(
                self.attachment_session(session)?,
                image,
                reply,
            ))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("生成原图写入未返回结果"))?
    }

    pub async fn read_generated(
        &self,
        session: SessionId,
        image: agentelier_tools::ImageDescriptor,
    ) -> io::Result<agentelier_tools::OriginalImage> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::ReadGenerated(
                self.attachment_session(session)?,
                image,
                reply,
            ))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("生成原图读取未返回结果"))?
    }
}

pub struct SessionStore {
    sender: mpsc::Sender<Command>,
    thread: JoinHandle<io::Result<()>>,
}

impl SessionStore {
    pub fn start(root: PathBuf) -> io::Result<Self> {
        let (sender, mut commands) = mpsc::channel(COMMAND_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("agentelier-session-store".into())
            .spawn(move || {
                debug!("会话存储工作者已启动");
                let mut writer = crate::journal::Writer::default();
                while let Some(command) = commands.blocking_recv() {
                    match command {
                        Command::Delete(session, reply) => { let _closed = reply.send(crate::deletion::delete(&root, session, &mut writer)); }
                        Command::ChangeMetadata(session, change, reply) => { let _closed = reply.send(writer.change_metadata(&root, session, change)); }
                        Command::GeneratedTitle(session, title, reply) => { let _closed = reply.send(writer.save_generated_title(&root, session, &title)); }
                        Command::Project(change, reply) => {
                            drop(reply.send(crate::projects::apply(&root, change)));
                        }
                        Command::ProjectDirectory(project, reply) => {
                            drop(reply.send(crate::projects::project_directory(&root, project)));
                        }
                        Command::Scan { task_history, reply } => {
                            let result = crate::reader::scan(&root, task_history);
                            if let Err(failure) = &result { error!(error = %failure, "本地会话目录读取失败"); }
                            if reply.send(result).is_err() { debug!("会话目录读取已结束，等待者已离开"); }
                        }
                        Command::MemoryCutoffs(reply) => {
                            let _closed = reply.send(crate::reader::memory_cutoffs(&root));
                        }
                        Command::MemorySessions(reply) => {
                            let _closed = reply.send(crate::reader::memory_sessions(&root));
                        }
                        Command::MemorySource(session, reply) => {
                            let _closed = reply.send(crate::reader::memory_source(&root, session));
                        }
                        Command::Load(session, reply) => {
                            let result = crate::reader::load(&root, session, &mut writer);
                            if let Err(failure) = &result { error!(session_id = %session, error = %failure, "本地会话读取失败"); }
                            if reply.send(result).is_err() { debug!("会话读取已结束，等待者已离开"); }
                        }
                        Command::LoadChild(session, agent, reply) => {
                            let result = crate::reader::load_child(&root, session, agent);
                            if let Err(failure) = &result { error!(session_id = %session, agent_id = %agent, error = %failure, "子 Agent 存档读取失败"); }
                            drop(reply.send(result));
                        }
                        Command::Append(event, reply) => {
                            let session = event.session();
                            let result = writer.append(&root, *event);
                            match &result {
                                Err(failure) => error!(session_id = %session, error = %failure, "会话记录追加失败"),
                                Ok(commit) => {
                                    if let Some(failure) = &commit.summary_failure { error!(session_id = %session, error = %failure, "会话记录已提交，摘要更新失败"); }
                                    debug!(session_id = %session, sequence = commit.sequence, "会话记录已同步提交");
                                }
                            }
                            if reply.send(result).is_err() { debug!("记录提交已结束，等待者已离开"); }
                        }
                        Command::SaveImage(reference, image, reply) => {
                            let result = crate::attachments::save(&root, &reference, &image);
                            if let Err(failure) = &result { error!(session_id = %reference.session, error = %failure, "会话图片附件提交失败"); }
                            else { debug!(session_id = %reference.session, sha256 = reference.image.digest, encoded_bytes = reference.image.encoded_bytes, "会话图片附件已提交并确认"); }
                            if reply.send(result).is_err() { debug!("附件提交已结束，等待者已离开"); }
                        }
                        Command::ReadImage(reference, reply) => {
                            let result = crate::attachments::read(&root, &reference);
                            if let Err(failure) = &result { error!(session_id = %reference.session, error = %failure, "会话图片附件读取失败"); }
                            if reply.send(result).is_err() { debug!("附件读取已结束，等待者已离开"); }
                        }
                        Command::SaveGenerated(session, image, reply) => {
                            let result = crate::attachments::save_generated(&root, session, &image);
                            match &result {
                                Ok(_) => debug!(
                                    session_id = %session, sha256 = image.descriptor().digest,
                                    encoded_bytes = image.descriptor().encoded_bytes, "会话生成原图已同步保存"
                                ),
                                Err(failure) => error!(session_id = %session, error = %failure, "会话生成原图提交失败"),
                            }
                            let _closed = reply.send(result);
                        }
                        Command::ReadGenerated(session, image, reply) => {
                            let _closed = reply.send(crate::attachments::read_generated(&root, session, &image));
                        }
                        Command::AudioDraft(session, audio, reply) => {
                            let result = (|| {
                                let attachment = if let Some(audio) = audio {
                                    crate::attachments::save_audio(&root, session, &audio)?;
                                    Some(audio.attachment)
                                } else { None };
                                let commit = writer.append(&root, RecordEvent::AudioDraft { session, attachment: attachment.clone() })?;
                                if let Some(failure) = commit.summary_failure {
                                    error!(session_id = %session, error = %failure, "会话草稿音频已提交，摘要更新失败");
                                }
                                Ok(attachment)
                            })();
                            let _closed = reply.send(result);
                        }
                        Command::ReadAudio(session, audio, reply) => {
                            let _closed = reply.send(crate::attachments::read_audio(&root, session, &audio));
                        }
                        Command::Create(initial, project, reply) => {
                            let result = create(&root, *initial, project);
                            if let Err(failure) = &result {
                                error!(error = %failure, "会话初始存档提交失败");
                            }
                            // A dropped waiter never cancels an admitted disk operation.
                            if reply.send(result).is_err() {
                                debug!("会话提交已结束，等待者已离开");
                            }
                        }
                    }
                }
                debug!("会话存储已排空并关闭");
                Ok(())
            })?;
        Ok(Self { sender, thread })
    }

    pub fn client(&self) -> SessionStoreClient {
        SessionStoreClient(self.sender.clone(), None)
    }

    pub async fn project_directory(&self, project: ProjectId) -> io::Result<PathBuf> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(Command::ProjectDirectory(project, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("项目存储未返回目录"))?
    }

    pub fn create(
        &self,
        initial: InitialSession,
        project: Option<ProjectId>,
    ) -> io::Result<oneshot::Receiver<io::Result<Commit>>> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .try_send(Command::Create(Box::new(initial), project, sender))
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "会话存储命令队列已满")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "会话存储工作者已关闭")
                }
            })?;
        Ok(receiver)
    }

    pub async fn finish(self) -> io::Result<()> {
        drop(self.sender);
        tokio::task::spawn_blocking(move || {
            self.thread
                .join()
                .map_err(|_| io::Error::other("会话存储工作者异常结束"))?
        })
        .await
        .map_err(|error| io::Error::other(format!("无法回收会话存储工作者：{error}")))?
    }
}

fn create(
    root: &std::path::Path,
    initial: InitialSession,
    project: Option<ProjectId>,
) -> io::Result<Commit> {
    initial.settings.instructions.validate()?;
    if project.is_some() && !initial.settings.kind.is_user() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "内部会话不能属于项目",
        ));
    }
    if initial.session.is_nil() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "初始会话身份为空",
        ));
    }
    let sessions = root.join("sessions");
    fs::create_dir_all(&sessions)?;
    let directory = sessions.join(initial.session.to_string());
    fs::create_dir(&directory)?;
    let summary = initial.summary(INITIAL_SEQUENCE);
    let record = Record {
        format: FORMAT.into(),
        version: FORMAT_VERSION,
        sequence: INITIAL_SEQUENCE,
        kind: RecordKind::Created(initial),
    };
    let journal = directory.join("session.jsonl");
    let mut membership_saved = false;
    let saved = (|| {
        if let Some(project) = project {
            crate::projects::save_membership(&directory, project)?;
            membership_saved = true;
        }
        let mut temporary = NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(temporary.as_file_mut(), &record)?;
        temporary.write_all(b"\n")?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(&journal)
            .map_err(|failure| failure.error)?;
        Ok::<(), io::Error>(())
    })();
    if let Err(failure) = saved {
        let cleanup = (|| {
            if membership_saved {
                fs::remove_file(directory.join(crate::projects::MEMBERSHIP_FILE))?;
            }
            fs::remove_dir(&directory)
        })();
        if let Err(cleanup) = cleanup {
            return Err(io::Error::other(format!(
                "初始记录未提交：{failure}；本次半成品目录清理失败：{cleanup}"
            )));
        }
        return Err(failure);
    }
    // The authoritative journal is already committed even if its projection fails.
    let summary_failure = (|| {
        let mut temporary = NamedTempFile::new_in(&directory)?;
        serde_json::to_writer(temporary.as_file_mut(), &summary)?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(directory.join("summary.json"))
            .map_err(|failure| failure.error)?;
        Ok::<(), io::Error>(())
    })()
    .err();
    Ok(Commit {
        session: summary.session,
        sequence: INITIAL_SEQUENCE,
        summary_failure,
    })
}
