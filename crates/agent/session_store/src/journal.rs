use crate::{Commit, FORMAT, FORMAT_VERSION, Record, RecordKind};
use agentelier_conversation::{RecordEvent, SessionId, SessionRecord, SessionSummary};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use tempfile::NamedTempFile;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const MAX_DEFAULT_TITLE_CHARACTERS: usize = 40;

struct Cursor {
    sequence: u64,
    length: u64,
    summary: SessionSummary,
    titled: bool,
    renamed: bool,
    poisoned: bool,
    kind: agentelier_conversation::SessionKind,
    scene_revisions: HashMap<SessionId, u64>,
}

#[derive(Default)]
pub(super) struct Writer(HashMap<SessionId, Cursor>);

fn update(cursor: &mut Cursor, record: &SessionRecord) {
    track_scene_revisions(&mut cursor.scene_revisions, &record.event);
    let input = match &record.event {
        RecordEvent::Accepted { state } => state.turn.as_ref().map(|turn| turn.input.as_str()),
        RecordEvent::QueueChanged {
            decision: agentelier_conversation::QueueDecision::Enqueued,
            state,
        } => state.queue.last().map(|input| input.text.as_str()),
        _ => None,
    };
    if !cursor.titled
        && let Some(text) = input
        && let Some(line) = text.lines().map(str::trim).find(|line| !line.is_empty())
    {
        cursor.summary.title = line.chars().take(MAX_DEFAULT_TITLE_CHARACTERS).collect();
        cursor.titled = true;
    }
    if input.is_some()
        || matches!(
            record.event,
            RecordEvent::SteerAccepted { .. } | RecordEvent::Finished { .. }
        )
    {
        cursor.summary.last_activity_ms = cursor.summary.last_activity_ms.max(record.at_ms);
    }
}

fn track_scene_revisions(revisions: &mut HashMap<SessionId, u64>, event: &RecordEvent) {
    let RecordEvent::ToolReturned {
        published_scene,
        scene_update,
        ..
    } = event
    else {
        return;
    };
    if let Some(scene) = published_scene {
        revisions.insert(scene.id, scene.revision);
    }
    if let Some(update) = scene_update
        && let Some(revision) = revisions.get_mut(&update.id)
        && *revision == update.expected_revision
        && let Some(next) = revision.checked_add(1)
    {
        *revision = next;
    }
}

pub(super) fn summarize(
    initial: &agentelier_conversation::InitialSession,
    records: &[SessionRecord],
    sequence: u64,
) -> (SessionSummary, bool) {
    let cursor = summarized_cursor(initial, records, sequence);
    (cursor.summary, cursor.titled)
}

fn summarized_cursor(
    initial: &agentelier_conversation::InitialSession,
    records: &[SessionRecord],
    sequence: u64,
) -> Cursor {
    let mut cursor = Cursor {
        sequence,
        length: 0,
        summary: initial.summary(sequence),
        titled: false,
        renamed: false,
        poisoned: false,
        kind: initial.settings.kind,
        scene_revisions: HashMap::new(),
    };
    for record in records {
        update(&mut cursor, record);
    }
    cursor
}

fn load_cursor(directory: &Path, session: SessionId) -> io::Result<Cursor> {
    let parsed = crate::reader::read(directory, session)?;
    if let Some(issue) = parsed.issue {
        return Err(io::Error::other(issue.to_string()));
    }
    let initial = parsed
        .initial
        .as_ref()
        .ok_or_else(|| io::Error::other("会话缺少初始记录"))?;
    let mut cursor = summarized_cursor(initial, &parsed.records, parsed.sequence);
    cursor.length = parsed.file_length;
    cursor.summary = parsed.summary;
    cursor.renamed = parsed.manual_title.is_some();
    cursor.titled |= cursor.renamed;
    Ok(cursor)
}

impl Writer {
    pub(super) fn reset(&mut self, session: SessionId) {
        self.0.remove(&session);
    }

    pub(super) fn append(&mut self, root: &Path, event: RecordEvent) -> io::Result<Commit> {
        let session = event.session();
        let at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_millis()
            .try_into()
            .map_err(io::Error::other)?;
        self.append_kind(
            root,
            session,
            RecordKind::Event(SessionRecord {
                sequence: 0,
                session,
                at_ms,
                event,
            }),
        )
    }

    pub(super) fn delete(&mut self, root: &Path, session: SessionId) -> io::Result<Commit> {
        self.append_kind(root, session, RecordKind::Deleted { session })
    }
    pub(super) fn change_metadata(
        &mut self,
        root: &Path,
        session: SessionId,
        change: agentelier_conversation::SessionMetadataChange,
    ) -> io::Result<crate::MetadataCommit> {
        use agentelier_conversation::SessionMetadataChange;
        let kind = match change {
            SessionMetadataChange::Rename(title) => RecordKind::Renamed {
                session,
                title: agentelier_conversation::session_title(&title).map_err(io::Error::other)?,
            },
            SessionMetadataChange::Archive => RecordKind::Archived { session },
        };
        let commit = self.append_kind(root, session, kind)?;
        let summary = self
            .0
            .get(&session)
            .ok_or_else(|| io::Error::other("元数据提交后缺少会话摘要"))?
            .summary
            .clone();
        Ok(crate::MetadataCommit {
            summary,
            summary_failure: commit.summary_failure,
        })
    }

    pub(super) fn save_generated_title(
        &mut self,
        root: &Path,
        session: SessionId,
        title: &str,
    ) -> io::Result<Option<Commit>> {
        let directory = crate::attachments::session_directory(root, session)?;
        let cursor = self.cursor(&directory, session)?;
        if !cursor.titled || cursor.renamed || cursor.summary.deleted || !cursor.kind.is_user() {
            return Ok(None);
        }
        let title = agentelier_conversation::session_title(title).map_err(io::Error::other)?;
        self.append_kind(root, session, RecordKind::Renamed { session, title })
            .map(Some)
    }

    fn cursor(&mut self, directory: &Path, session: SessionId) -> io::Result<&mut Cursor> {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(session) {
            entry.insert(load_cursor(directory, session)?);
        }
        self.0
            .get_mut(&session)
            .ok_or_else(|| io::Error::other("会话写入位置未初始化"))
    }

    fn append_kind(
        &mut self,
        root: &Path,
        session: SessionId,
        kind: RecordKind,
    ) -> io::Result<Commit> {
        let directory = crate::attachments::session_directory(root, session)?;
        let cursor = self.cursor(&directory, session)?;
        if !cursor.kind.is_user()
            && matches!(
                kind,
                RecordKind::Renamed { .. }
                    | RecordKind::Deleted { .. }
                    | RecordKind::Archived { .. }
            )
        {
            return Err(io::Error::other("普通聊天操作不能更改内部会话"));
        }
        if cursor.poisoned {
            return Err(io::Error::other("会话已有未确认的写入，须恢复后才能继续"));
        }
        let sequence = cursor
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("会话记录序号已耗尽"))?;
        if cursor.summary.deleted {
            return Err(io::Error::other("会话已删除"));
        }
        if let RecordKind::Event(record) = &kind
            && let RecordEvent::SceneState {
                artifact, state, ..
            } = &record.event
        {
            state.validate().map_err(io::Error::other)?;
            if !cursor.kind.is_user()
                || cursor.scene_revisions.get(artifact) != Some(&state.revision)
            {
                return Err(io::Error::other("保存的产物定义修订已改变"));
            }
        }
        let record = Record {
            format: FORMAT.into(),
            version: FORMAT_VERSION,
            sequence,
            kind,
        };
        let mut bytes = serde_json::to_vec(&record)?;
        bytes.push(b'\n');
        let mut file = OpenOptions::new()
            .append(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(directory.join("session.jsonl"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || metadata.len() != cursor.length
        {
            cursor.poisoned = true;
            return Err(io::Error::other("会话日志在单写入者之外发生变化，拒绝追加"));
        }
        if let Err(failure) = file
            .write_all(&bytes)
            .and_then(|()| file.flush())
            .and_then(|()| file.sync_all())
        {
            cursor.poisoned = true;
            return Err(failure);
        }
        cursor.sequence = sequence;
        cursor.length = cursor
            .length
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| io::Error::other("会话记录长度超出范围"))?;
        cursor.summary.last_sequence = sequence;
        match record.kind {
            RecordKind::Event(event) => update(cursor, &event),
            RecordKind::Renamed { title, .. } => {
                cursor.summary.title = title;
                cursor.titled = true;
                cursor.renamed = true;
            }
            RecordKind::Archived { .. } => cursor.summary.archived = true,
            RecordKind::Deleted { .. } => cursor.summary.deleted = true,
            RecordKind::Created(_) => return Err(io::Error::other("追加路径不能创建初始记录")),
        }
        Ok(Commit {
            session,
            sequence,
            summary_failure: write_summary(&directory, &cursor.summary).err(),
        })
    }
}

pub(super) fn write_summary(directory: &Path, summary: &SessionSummary) -> io::Result<()> {
    let path = directory.join("summary.json");
    match fs::symlink_metadata(&path) {
        Ok(metadata)
            if !metadata.is_file()
                || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 =>
        {
            return Err(io::Error::other("会话摘要不是普通文件"));
        }
        Ok(_) => {}
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure),
    }
    let mut temporary = NamedTempFile::new_in(directory)?;
    serde_json::to_writer(temporary.as_file_mut(), summary)?;
    temporary.as_file_mut().flush()?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|failure| failure.error)?;
    Ok(())
}
