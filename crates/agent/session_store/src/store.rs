use agentelier_conversation::{
    AudioAttachment, ComposerContent, InitialSession, InputImage, ProjectId, RecordEvent,
    STANDALONE_PROJECT_ID, SessionId, SessionRecord, StoredImage,
};
use agentelier_tools::ImagePayload;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;
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
    pub background_tasks: Vec<agentelier_conversation::BackgroundTask>,
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
    StageInput(
        SessionId,
        crate::composer::Imported,
        oneshot::Sender<io::Result<ComposerContent>>,
    ),
    DraftInputImage(
        SessionId,
        InputImage,
        oneshot::Sender<io::Result<Arc<ImagePayload>>>,
    ),
    UserInputImage(
        SessionId,
        InputImage,
        oneshot::Sender<io::Result<Arc<ImagePayload>>>,
    ),
    DraftInputAudio(
        SessionId,
        AudioAttachment,
        oneshot::Sender<io::Result<Vec<u8>>>,
    ),
    AudioDraft(
        SessionId,
        Option<crate::audio::ImportedAudio>,
        oneshot::Sender<io::Result<Option<AudioAttachment>>>,
    ),
    SavePublishedAudio(
        SessionId,
        crate::audio::ImportedAudio,
        oneshot::Sender<io::Result<AudioAttachment>>,
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
    SessionProject(SessionId, oneshot::Sender<io::Result<Option<ProjectId>>>),
    Scan {
        task_history: bool,
        reply: oneshot::Sender<io::Result<ArchiveCatalog>>,
    },
    MemorySessions(oneshot::Sender<io::Result<Vec<SessionId>>>),
    MemoryCutoffs(oneshot::Sender<io::Result<Vec<crate::MemoryCutoff>>>),
    MemorySource(
        SessionId,
        oneshot::Sender<io::Result<Option<crate::reader::MemoryJournal>>>,
    ),
    Load(
        SessionId,
        Option<SessionId>,
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
        Option<Box<RecordEvent>>,
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
    pub async fn stage_input(
        &self,
        owner: SessionId,
        path: PathBuf,
        image: bool,
    ) -> io::Result<ComposerContent> {
        if owner.is_nil() {
            return Err(io::Error::other("附件草稿缺少身份"));
        }
        let imported = tokio::task::spawn_blocking(move || {
            if image {
                crate::composer::Imported::image(&path)
            } else {
                crate::audio::import(&path, crate::audio::Source::Draft)
                    .map(crate::composer::Imported::Audio)
            }
        })
        .await
        .map_err(io::Error::other)??;
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::StageInput(owner, imported, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("未收到草稿附件回执"))?
    }

    pub async fn input_image(
        &self,
        session: SessionId,
        image: InputImage,
        draft: bool,
    ) -> io::Result<Arc<ImagePayload>> {
        let (reply, result) = oneshot::channel();
        let command = if draft {
            Command::DraftInputImage(session, image, reply)
        } else {
            Command::UserInputImage(session, image, reply)
        };
        self.0
            .send(command)
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("未收到输入图片读取回执"))?
    }

    pub async fn draft_input_audio(
        &self,
        owner: SessionId,
        audio: AudioAttachment,
    ) -> io::Result<Vec<u8>> {
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::DraftInputAudio(owner, audio, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("未收到草稿音频读取回执"))?
    }

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
                tokio::task::spawn_blocking(move || {
                    crate::audio::import(&path, crate::audio::Source::Draft)
                })
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

    pub async fn save_published_audio(
        &self,
        session: SessionId,
        path: PathBuf,
    ) -> io::Result<AudioAttachment> {
        let session = self.attachment_session(session)?;
        let audio = tokio::task::spawn_blocking(move || {
            crate::audio::import(&path, crate::audio::Source::Published)
        })
        .await
        .map_err(io::Error::other)??;
        let (reply, result) = oneshot::channel();
        self.0
            .send(Command::SavePublishedAudio(session, audio, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("助手音频保存未返回结果"))?
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
        let journal = result
            .await
            .map_err(|_| io::Error::other("记忆来源未返回日志快照"))??;
        let Some(journal) = journal else {
            return Ok(None);
        };
        tokio::task::spawn_blocking(move || journal.read())
            .await
            .map_err(io::Error::other)?
    }

    pub fn load(
        &self,
        session: SessionId,
    ) -> io::Result<oneshot::Receiver<io::Result<agentelier_conversation::RestoredSession>>> {
        self.load_scoped(session, None)
    }

    pub fn load_background(
        &self,
        session: SessionId,
        source: SessionId,
    ) -> io::Result<oneshot::Receiver<io::Result<agentelier_conversation::RestoredSession>>> {
        self.load_scoped(session, Some(source))
    }

    fn load_scoped(
        &self,
        session: SessionId,
        source: Option<SessionId>,
    ) -> io::Result<oneshot::Receiver<io::Result<agentelier_conversation::RestoredSession>>> {
        let (reply, result) = oneshot::channel();
        self.0
            .try_send(Command::Load(session, source, reply))
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
    root: Arc<PathBuf>,
    sender: mpsc::Sender<Command>,
    thread: JoinHandle<io::Result<()>>,
}

impl SessionStore {
    pub fn start(root: PathBuf) -> io::Result<Self> {
        let (sender, mut commands) = mpsc::channel(COMMAND_CAPACITY);
        let owner_root = Arc::new(root.clone());
        let thread = std::thread::Builder::new()
            .name("agentelier-session-store".into())
            .spawn(move || {
                debug!("会话存储工作者已启动");
                let mut writer = crate::journal::Writer::default();
                let mut drafts = crate::composer::Drafts::default();
                while let Some(command) = commands.blocking_recv() {
                    match command {
                        Command::StageInput(owner, media, reply) => {
                            drop(reply.send(drafts.stage(&root, owner, media)));
                        }
                        Command::DraftInputImage(owner, image, reply) => {
                            drop(reply.send(drafts.read_image(owner, &image)));
                        }
                        Command::UserInputImage(session, image, reply) => {
                            drop(reply.send(crate::composer::read_image(&root, session, &image)));
                        }
                        Command::DraftInputAudio(owner, audio, reply) => {
                            drop(reply.send(drafts.read_audio(owner, &audio)));
                        }
                        Command::Delete(session, reply) => { let _closed = reply.send(crate::deletion::delete(&root, session, &mut writer)); }
                        Command::ChangeMetadata(session, change, reply) => { let _closed = reply.send(writer.change_metadata(&root, session, change)); }
                        Command::GeneratedTitle(session, title, reply) => { let _closed = reply.send(writer.save_generated_title(&root, session, &title)); }
                        Command::Project(change, reply) => {
                            drop(reply.send(crate::projects::apply(&root, change)));
                        }
                        Command::ProjectDirectory(project, reply) => {
                            drop(reply.send(crate::projects::project_directory(&root, project)));
                        }
                        Command::SessionProject(session, reply) => {
                            drop(reply.send(crate::projects::session_project(&root, session)));
                        }
                        Command::Scan { task_history, reply } => {
                            let cleanup_errors = crate::deletion::resume_groups(&root, &mut writer);
                            let result = crate::reader::scan(&root, task_history).map(|mut catalog| {
                                catalog.cleanup_errors.extend(cleanup_errors);
                                catalog
                            });
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
                            let _closed = reply.send(crate::reader::MemoryJournal::open(&root, session));
                        }
                        Command::Load(session, source, reply) => {
                            let result = crate::reader::load(&root, session, source, &mut writer);
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
                            let result = (|| {
                                if crate::composer::admitted_input(&event).is_some() {
                                    let directory = crate::attachments::session_directory(&root, session)?;
                                    drafts.bind(&directory, &event)?;
                                }
                                writer.append(&root, *event)
                            })();
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
                        Command::SavePublishedAudio(session, audio, reply) => {
                            let result = crate::attachments::save_audio(&root, session, &audio)
                                .map(|()| audio.attachment);
                            let _closed = reply.send(result);
                        }
                        Command::Create(initial, project, first, reply) => {
                            let result = create(&root, *initial, project, first.map(|event| *event), &drafts);
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
        Ok(Self {
            root: owner_root,
            sender,
            thread,
        })
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

    pub async fn session_project(&self, session: SessionId) -> io::Result<Option<ProjectId>> {
        let (reply, result) = oneshot::channel();
        self.sender
            .send(Command::SessionProject(session, reply))
            .await
            .map_err(|_| io::Error::other("会话存储工作者已关闭"))?;
        result
            .await
            .map_err(|_| io::Error::other("项目存储未返回会话归属"))?
    }

    pub fn publish(
        &self,
        initial: InitialSession,
        project: Option<ProjectId>,
        first: Option<RecordEvent>,
    ) -> io::Result<Publication> {
        let session = initial.session;
        let turn = match &first {
            Some(RecordEvent::Accepted { state }) => {
                state.turn.as_ref().map(|turn| (turn.id, turn.operation_id))
            }
            _ => None,
        };
        let (sender, receiver) = oneshot::channel();
        self.sender
            .try_send(Command::Create(
                Box::new(initial),
                project,
                first.map(Box::new),
                sender,
            ))
            .map_err(|failure| match failure {
                mpsc::error::TrySendError::Full(_) => {
                    io::Error::new(io::ErrorKind::WouldBlock, "会话存储命令队列已满")
                }
                mpsc::error::TrySendError::Closed(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "会话存储工作者已关闭")
                }
            })?;
        Ok(Publication {
            root: self.root.clone(),
            session,
            turn,
            receiver,
        })
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

pub struct Publication {
    root: Arc<PathBuf>,
    session: SessionId,
    turn: Option<(agentelier_conversation::TurnId, u64)>,
    receiver: oneshot::Receiver<io::Result<Commit>>,
}

impl Publication {
    pub async fn wait(&mut self) -> io::Result<Commit> {
        if let Ok(result) = (&mut self.receiver).await {
            return result;
        }
        let root = self.root.clone();
        let session = self.session;
        let turn = self.turn;
        tokio::task::spawn_blocking(move || {
            let directory = crate::attachments::session_directory(&root, session)?;
            let parsed = crate::reader::read(&directory, session)?;
            let sequence = if let Some((id, operation)) = turn {
                parsed.records.first().filter(|record| {
                    matches!(&record.event, RecordEvent::Accepted { state } if state.turn.as_ref().is_some_and(|turn| {
                        turn.id == id && turn.operation_id == operation
                    }))
                }).map(|record| record.sequence)
            } else {
                parsed.initial.as_ref().map(|_| INITIAL_SEQUENCE)
            };
            let sequence = sequence.ok_or_else(|| io::Error::other("未找到与发布请求一致的会话记录"))?;
            Ok(Commit { session, sequence, summary_failure: None })
        }).await.map_err(io::Error::other)?
    }
}

fn create(
    root: &std::path::Path,
    initial: InitialSession,
    project: Option<ProjectId>,
    first: Option<RecordEvent>,
    drafts: &crate::composer::Drafts,
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
    if let Some(binding) = &initial.settings.background {
        let source = crate::attachments::session_directory(root, binding.source)?;
        let source = crate::reader::read(&source, binding.source)?;
        if source.summary.deleted
            || crate::deletion::group_pending(root, binding.source)
            || source.summary.archived
            || source.issue.is_some()
            || source.initial.as_ref().is_none_or(|initial| {
                !initial.settings.kind.is_user() || initial.settings.background.is_some()
            })
            || project.is_none()
            || project != crate::projects::session_project(root, binding.source)?
        {
            return Err(io::Error::other(
                "后台任务的来源聊天不可用，或项目归属不一致",
            ));
        }
    }
    let first = first
        .map(|event| {
            match &event {
                RecordEvent::Accepted { state }
                    if state.session == initial.session
                        && state
                            .turn
                            .as_ref()
                            .is_some_and(|turn| !turn.is_retry && !turn.result_continuation) => {}
                _ => return Err(io::Error::other("会话初始提交的首条消息无效")),
            }
            Ok(SessionRecord {
                sequence: INITIAL_SEQUENCE + 1,
                session: initial.session,
                at_ms: crate::journal::timestamp()?,
                event,
            })
        })
        .transpose()?;
    let sequence = first
        .as_ref()
        .map_or(INITIAL_SEQUENCE, |first| first.sequence);
    let (summary, _) = crate::journal::summarize(&initial, first.as_slice(), sequence);
    let kind = initial.settings.kind;
    let sessions = root.join("sessions");
    fs::create_dir_all(&sessions)?;
    let directory = sessions.join(initial.session.to_string());
    let unpublished = tempfile::Builder::new()
        .prefix(".publishing-")
        .tempdir_in(&sessions)?;
    let staging = fs::canonicalize(unpublished.path())?;
    let record = Record {
        format: FORMAT.into(),
        version: FORMAT_VERSION,
        sequence: INITIAL_SEQUENCE,
        kind: RecordKind::Created(initial),
    };
    if kind.is_user() {
        crate::projects::save_membership(&staging, project.unwrap_or(STANDALONE_PROJECT_ID))?;
    }
    let mut journal = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(staging.join("session.jsonl"))?;
    serde_json::to_writer(&mut journal, &record)?;
    journal.write_all(b"\n")?;
    if let Some(first) = first {
        drafts.bind(&staging, &first.event)?;
        let record = Record {
            format: FORMAT.into(),
            version: FORMAT_VERSION,
            sequence: first.sequence,
            kind: RecordKind::Event(first),
        };
        serde_json::to_writer(&mut journal, &record)?;
        journal.write_all(b"\n")?;
    }
    journal.sync_all()?;
    drop(journal);
    fs::rename(&staging, &directory)?;
    // The authoritative journal is already committed even if its projection fails.
    let summary_failure = crate::journal::write_summary(&directory, &summary, kind, false).err();
    Ok(Commit {
        session: summary.session,
        sequence,
        summary_failure,
    })
}
