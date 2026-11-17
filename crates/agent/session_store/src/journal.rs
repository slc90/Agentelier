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
    poisoned: bool,
}

#[derive(Default)]
pub(super) struct Writer(HashMap<SessionId, Cursor>);

fn update(cursor: &mut Cursor, record: &SessionRecord) {
    let state = record.event.state();
    let input = match &record.event {
        RecordEvent::Accepted { .. } => state.turn.as_ref().map(|turn| turn.input.as_str()),
        RecordEvent::QueueChanged {
            decision: agentelier_conversation::QueueDecision::Enqueued,
            ..
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

pub(super) fn summarize(
    initial: &agentelier_conversation::InitialSession,
    records: &[SessionRecord],
    sequence: u64,
) -> (SessionSummary, bool) {
    let mut cursor = Cursor {
        sequence,
        length: 0,
        summary: initial.summary(sequence),
        titled: false,
        poisoned: false,
    };
    for record in records {
        update(&mut cursor, record);
    }
    (cursor.summary, cursor.titled)
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
    let (_, titled) = summarize(initial, &parsed.records, parsed.sequence);
    Ok(Cursor {
        sequence: parsed.sequence,
        length: parsed.file_length,
        summary: parsed.summary,
        titled: titled || parsed.manual_title.is_some(),
        poisoned: false,
    })
}

impl Writer {
    pub(super) fn reset(&mut self, session: SessionId) {
        self.0.remove(&session);
    }

    pub(super) fn append(&mut self, root: &Path, event: RecordEvent) -> io::Result<Commit> {
        let session = event.state().session;
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
                session,
                at_ms,
                event,
            }),
        )
    }

    pub(super) fn delete(&mut self, root: &Path, session: SessionId) -> io::Result<Commit> {
        self.append_kind(root, session, RecordKind::Deleted { session })
    }
    pub(super) fn rename(
        &mut self,
        root: &Path,
        session: SessionId,
        title: String,
    ) -> io::Result<crate::RenameCommit> {
        let title = agentelier_conversation::session_title(&title).map_err(io::Error::other)?;
        let commit = self.append_kind(root, session, RecordKind::Renamed { session, title })?;
        let summary = self
            .0
            .get(&session)
            .ok_or_else(|| io::Error::other("名称提交后缺少会话摘要"))?
            .summary
            .clone();
        Ok(crate::RenameCommit {
            summary,
            summary_failure: commit.summary_failure,
        })
    }

    fn append_kind(
        &mut self,
        root: &Path,
        session: SessionId,
        kind: RecordKind,
    ) -> io::Result<Commit> {
        let directory = crate::attachments::session_directory(root, session)?;
        if let std::collections::hash_map::Entry::Vacant(entry) = self.0.entry(session) {
            entry.insert(load_cursor(&directory, session)?);
        }
        let cursor = self
            .0
            .get_mut(&session)
            .ok_or_else(|| io::Error::other("会话写入位置未初始化"))?;
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
            }
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
