use crate::{Commit, FORMAT, FORMAT_VERSION, Record, RecordKind};
use agentelier_conversation::{
    ArchiveEntry, InitialSession, RecordEvent, RestoredSession, SessionId, SessionRecord,
    SessionSummary,
};
use sha2::{Digest, Sha256};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;
use std::{fs, io};
use tempfile::{Builder, NamedTempFile};

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

pub(super) enum Issue {
    Tail(u64),
    Corrupt { offset: u64, message: String },
}

impl std::fmt::Display for Issue {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tail(offset) => write!(formatter, "会话日志在字节 {offset} 有未完成末行"),
            Self::Corrupt { offset, message } => {
                write!(formatter, "会话日志在字节 {offset} 损坏：{message}")
            }
        }
    }
}

pub(super) struct Parsed {
    pub(super) initial: Option<InitialSession>,
    pub(super) records: Vec<SessionRecord>,
    pub(super) summary: SessionSummary,
    pub(super) manual_title: Option<String>,
    deleted: bool,
    pub(super) sequence: u64,
    pub(super) file_length: u64,
    valid_length: u64,
    prefix_digest: [u8; 32],
    pub(super) issue: Option<Issue>,
}

fn placeholder(session: SessionId) -> SessionSummary {
    SessionSummary {
        session,
        title: "无法读取的会话".into(),
        created_at_ms: 0,
        last_activity_ms: 0,
        last_sequence: 0,
        deleted: false,
    }
}

pub(super) fn read(directory: &Path, session: SessionId) -> io::Result<Parsed> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.join("session.jsonl"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("会话日志不是普通文件"));
    }
    let mut reader = BufReader::new(file);
    let mut parsed = Parsed {
        initial: None,
        records: Vec::new(),
        summary: placeholder(session),
        manual_title: None,
        deleted: false,
        sequence: 0,
        file_length: metadata.len(),
        valid_length: 0,
        prefix_digest: [0; 32],
        issue: None,
    };
    let mut line = Vec::new();
    let mut digest = Sha256::new();
    while reader.read_until(b'\n', &mut line)? != 0 {
        let offset = parsed.valid_length;
        if !line.ends_with(b"\n") {
            parsed.issue = Some(Issue::Tail(offset));
            break;
        }
        let record = match serde_json::from_slice::<Record>(&line) {
            Ok(record) => record,
            Err(failure) => {
                parsed.issue = Some(Issue::Corrupt {
                    offset,
                    message: format!(
                        "JSON {:?}，行 {}、列 {}",
                        failure.classify(),
                        failure.line(),
                        failure.column()
                    ),
                });
                break;
            }
        };
        let outcome = (|| {
            if record.format != FORMAT || record.version != FORMAT_VERSION {
                return Err(io::Error::other("不支持的会话文件格式"));
            }
            if parsed.sequence.checked_add(1) != Some(record.sequence) {
                return Err(io::Error::other("会话记录序号不连续或重复"));
            }
            if parsed.deleted {
                return Err(io::Error::other("已删除会话包含后续记录"));
            }
            match record.kind {
                RecordKind::Deleted { session: target }
                    if parsed.initial.is_some() && target == session =>
                {
                    parsed.deleted = true
                }
                RecordKind::Created(initial)
                    if parsed.initial.is_none()
                        && record.sequence == crate::store::INITIAL_SEQUENCE
                        && initial.session == session =>
                {
                    initial.settings.instructions.validate()?;
                    parsed.initial = Some(initial);
                }
                RecordKind::Renamed {
                    session: target,
                    title,
                } if parsed.initial.is_some() && target == session => {
                    let normalized =
                        agentelier_conversation::session_title(&title).map_err(io::Error::other)?;
                    if normalized != title {
                        return Err(io::Error::other("存档名称不是有效单行标题"));
                    }
                    parsed.manual_title = Some(title);
                }
                RecordKind::Event(event)
                    if parsed.initial.is_some()
                        && event.session == session
                        && event.event.state().session == session =>
                {
                    parsed.records.push(event)
                }
                _ => return Err(io::Error::other("会话初始记录、事件身份或顺序不一致")),
            }
            parsed.sequence = record.sequence;
            Ok::<(), io::Error>(())
        })();
        if let Err(failure) = outcome {
            parsed.issue = Some(Issue::Corrupt {
                offset,
                message: failure.to_string(),
            });
            break;
        }
        parsed.valid_length = parsed
            .valid_length
            .checked_add(line.len() as u64)
            .ok_or_else(|| io::Error::other("日志有效前缀长度超出范围"))?;
        digest.update(&line);
        line.clear();
    }
    parsed.prefix_digest = digest.finalize().into();
    if let Some(initial) = &parsed.initial {
        parsed.summary = crate::journal::summarize(initial, &parsed.records, parsed.sequence).0;
        parsed.summary.deleted = parsed.deleted;
        if let Some(title) = &parsed.manual_title {
            parsed.summary.title = title.clone();
        }
    } else if parsed.issue.is_none() {
        parsed.issue = Some(Issue::Corrupt {
            offset: 0,
            message: "会话没有有效初始记录".into(),
        });
    }
    Ok(parsed)
}

fn cached_summary(directory: &Path) -> io::Result<SessionSummary> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(directory.join("summary.json"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("会话摘要不是普通文件"));
    }
    serde_json::from_reader(file).map_err(|failure: serde_json::Error| {
        if failure.is_io() {
            io::Error::other(failure)
        } else {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "摘要 JSON {:?}，行 {}、列 {}",
                    failure.classify(),
                    failure.line(),
                    failure.column()
                ),
            )
        }
    })
}

pub(super) fn scan(root: &Path) -> io::Result<crate::ArchiveCatalog> {
    let root = fs::canonicalize(root)?;
    let path = root.join("sessions");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {
            return Ok(crate::ArchiveCatalog {
                entries: Vec::new(),
                cleanup_errors: Vec::new(),
            });
        }
        Err(failure) => return Err(failure),
    };
    if !metadata.is_dir()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || fs::canonicalize(&path)?.parent() != Some(root.as_path())
    {
        return Err(io::Error::other("会话目录包含重解析点或超出数据目录"));
    }
    let mut entries = Vec::new();
    let mut cleanup_errors = Vec::new();
    for entry in fs::read_dir(&path)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if let Some(id) = name.strip_prefix(crate::deletion::PENDING_PREFIX) {
            if let Ok(session) = SessionId::parse_str(id)
                && !session.is_nil()
                && session.to_string() == id
                && let Err(failure) = crate::deletion::resume(&root, session)
            {
                cleanup_errors.push(format!(
                    "会话 {session} 已删除，但本地清理未完成：{failure}"
                ));
            }
            continue;
        }
        let Ok(session) = SessionId::parse_str(&name) else {
            continue;
        };
        if session.is_nil() || session.to_string() != name {
            continue;
        }
        let result = crate::attachments::session_directory(&root, session)
            .and_then(|directory| read(&directory, session).map(|parsed| (directory, parsed)));
        match result {
            Ok((directory, parsed)) => {
                let mut error = parsed.issue.as_ref().map(ToString::to_string);
                if parsed.issue.is_none() {
                    let cached = cached_summary(&directory);
                    if cached.as_ref().ok() != Some(&parsed.summary) {
                        match crate::journal::write_summary(&directory, &parsed.summary) {
                            Ok(()) => {
                                if let Err(failure) = cached {
                                    tracing::info!(session_id = %session, reason = %failure, "会话摘要读取失败后已从权威记录重建");
                                }
                            }
                            Err(failure) => {
                                error = Some(match cached {
                                    Ok(_) => format!("历史可读，但摘要更新失败：{failure}"),
                                    Err(read_failure) => {
                                        format!("摘要读取失败：{read_failure}；重建失败：{failure}")
                                    }
                                })
                            }
                        }
                    }
                }
                if parsed.summary.deleted {
                    if let Err(failure) = crate::deletion::cleanup(&root, session) {
                        cleanup_errors.push(format!(
                            "会话 {session} 已删除，但本地清理未完成：{failure}"
                        ));
                    }
                } else {
                    entries.push(ArchiveEntry {
                        summary: parsed.summary,
                        error,
                    });
                }
            }
            Err(failure) => entries.push(ArchiveEntry {
                summary: placeholder(session),
                error: Some(failure.to_string()),
            }),
        }
    }
    entries.sort_by(|left, right| {
        right
            .summary
            .last_activity_ms
            .cmp(&left.summary.last_activity_ms)
            .then_with(|| {
                left.summary
                    .session
                    .as_bytes()
                    .cmp(right.summary.session.as_bytes())
            })
    });
    Ok(crate::ArchiveCatalog {
        entries,
        cleanup_errors,
    })
}

fn repair(
    root: &Path,
    session: SessionId,
    parsed: &Parsed,
    event: RecordEvent,
) -> io::Result<Commit> {
    let directory = crate::attachments::session_directory(root, session)?;
    let path = directory.join("session.jsonl");
    let mut original = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&path)?;
    let metadata = original.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() != parsed.file_length
    {
        return Err(io::Error::other("原会话日志已变化，未修复"));
    }
    let mut backup = Builder::new()
        .prefix("session-before-recovery-")
        .suffix(".jsonl")
        .tempfile_in(&directory)?;
    if io::copy(&mut original, backup.as_file_mut())? != parsed.file_length {
        return Err(io::Error::other("原会话日志长度已变化，未修复"));
    }
    backup.as_file_mut().flush()?;
    backup.as_file().sync_all()?;
    let (mut backup_file, backup_path) = backup.keep().map_err(|failure| failure.error)?;
    let sequence = parsed
        .sequence
        .checked_add(1)
        .ok_or_else(|| io::Error::other("恢复记录序号已耗尽"))?;
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(io::Error::other)?
        .as_millis()
        .try_into()
        .map_err(io::Error::other)?;
    let record = Record {
        format: FORMAT.into(),
        version: FORMAT_VERSION,
        sequence,
        kind: RecordKind::Event(SessionRecord {
            session,
            at_ms,
            event,
        }),
    };
    let mut replacement = NamedTempFile::new_in(&directory)?;
    backup_file.seek(SeekFrom::Start(0))?;
    const COPY_BUFFER_BYTES: usize = 64 * 1024;
    let mut buffer = [0; COPY_BUFFER_BYTES];
    let mut remaining = parsed.valid_length;
    let mut digest = Sha256::new();
    while remaining != 0 {
        let limit = remaining.min(COPY_BUFFER_BYTES as u64) as usize;
        let length = backup_file.read(&mut buffer[..limit])?;
        if length == 0 {
            return Err(io::Error::other("备份中的有效前缀不完整，未修复"));
        }
        digest.update(&buffer[..length]);
        replacement.write_all(&buffer[..length])?;
        remaining -= length as u64;
    }
    if <[u8; 32]>::from(digest.finalize()) != parsed.prefix_digest {
        return Err(io::Error::other("原会话日志前缀已变化，原件已保留，未修复"));
    }
    drop(backup_file);
    serde_json::to_writer(replacement.as_file_mut(), &record)?;
    replacement.write_all(b"\n")?;
    replacement.as_file_mut().flush()?;
    replacement.as_file().sync_all()?;
    drop(original);
    replacement.persist(&path).map_err(|failure| {
        io::Error::other(format!(
            "原件已保留于 {}，恢复文件提交失败：{}",
            backup_path.display(),
            failure.error
        ))
    })?;
    tracing::info!(session_id = %session, backup = %backup_path.display(), "已保留残缺日志原件并恢复有效记录");
    let mut summary = parsed.summary.clone();
    summary.last_sequence = sequence;
    Ok(Commit {
        session,
        sequence,
        summary_failure: crate::journal::write_summary(&directory, &summary).err(),
    })
}

pub(super) fn load(
    root: &Path,
    session: SessionId,
    writer: &mut crate::journal::Writer,
) -> io::Result<RestoredSession> {
    let directory = crate::attachments::session_directory(root, session)?;
    let mut parsed = read(&directory, session)?;
    if let Some(issue @ Issue::Corrupt { .. }) = &parsed.issue {
        return Err(io::Error::other(issue.to_string()));
    }
    if parsed.summary.deleted {
        return Err(io::Error::other("会话已删除"));
    }
    let initial = parsed
        .initial
        .take()
        .ok_or_else(|| io::Error::other("存档没有可恢复的初始记录"))?;
    let mut restored = agentelier_conversation::restore(
        initial,
        std::mem::take(&mut parsed.records),
        |reference| {
            crate::attachments::read(root, reference).map_err(|failure| {
                agentelier_conversation::ConversationFailure::new(failure.to_string())
            })
        },
    )
    .map_err(io::Error::other)?;
    if matches!(parsed.issue, Some(Issue::Tail(_))) {
        restored.add_recovery_notice("日志尾部残缺，已保留原件并恢复有效记录");
        let commit = repair(root, session, &parsed, restored.recovery_event())?;
        writer.reset(session);
        if let Some(failure) = commit.summary_failure {
            tracing::error!(session_id = %session, error = %failure, "日志尾部已恢复，摘要更新失败");
        }
    } else if restored.normalized {
        writer.reset(session);
        let commit = writer.append(root, restored.recovery_event())?;
        if let Some(failure) = commit.summary_failure {
            tracing::error!(session_id = %session, error = %failure, "中断历史已规范化，摘要更新失败");
        }
    }
    Ok(restored)
}
