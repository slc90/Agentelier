use agentelier_conversation::SessionId;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

const REPARSE_POINT: u32 = 0x0000_0400;
pub(super) const PENDING_PREFIX: &str = ".deleting-";
const GROUP_PREFIX: &str = ".deleting-group-";

#[derive(Serialize, Deserialize)]
struct Group {
    source: SessionId,
    tasks: Vec<SessionId>,
}

fn group_path(root: &Path, source: SessionId) -> PathBuf {
    root.join("sessions")
        .join(format!("{GROUP_PREFIX}{source}.json"))
}

pub(super) fn group_pending(root: &Path, source: SessionId) -> bool {
    group_path(root, source).is_file()
}

fn linked(root: &Path, source: SessionId) -> io::Result<Vec<SessionId>> {
    let mut tasks = Vec::new();
    for entry in fs::read_dir(root.join("sessions"))? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(session) = name
            .to_str()
            .and_then(|name| SessionId::parse_str(name).ok())
        else {
            continue;
        };
        if name.to_str() != Some(session.to_string().as_str())
            || session == source
            || session.is_nil()
        {
            continue;
        }
        let path = crate::attachments::session_directory(root, session)?;
        let parsed = crate::reader::read(&path, session)?;
        if parsed.summary.background_source == Some(source) {
            if let Some(issue) = parsed.issue {
                return Err(io::Error::other(issue.to_string()));
            }
            tasks.push(session);
        }
    }
    tasks.sort_unstable();
    Ok(tasks)
}

fn write_group(root: &Path, group: &Group) -> io::Result<()> {
    let directory = root.join("sessions");
    let mut temporary = tempfile::NamedTempFile::new_in(&directory)?;
    serde_json::to_writer(temporary.as_file_mut(), group)?;
    temporary.write_all(b"\n")?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(group_path(root, group.source))
        .map_err(|failure| failure.error)?;
    Ok(())
}

fn complete_group(
    root: &Path,
    source: SessionId,
    writer: &mut crate::journal::Writer,
) -> io::Result<()> {
    let path = group_path(root, source);
    regular(&path)?;
    let group: Group = serde_json::from_slice(&fs::read(&path)?)?;
    if group.source != source
        || source.is_nil()
        || group
            .tasks
            .iter()
            .any(|task| task.is_nil() || *task == source)
    {
        return Err(io::Error::other("成组删除记录的会话身份无效"));
    }
    for task in group.tasks.into_iter().chain([source]) {
        match crate::attachments::session_directory(root, task) {
            Ok(directory) => {
                let parsed = crate::reader::read(&directory, task)?;
                let expected = (task != source).then_some(source);
                if parsed.summary.background_source != expected {
                    return Err(io::Error::other("待清理会话归属与删除记录不一致"));
                }
                if let Some(error) = delete_one(root, task, writer)?.cleanup_error {
                    return Err(error);
                }
            }
            Err(failure) if failure.kind() == io::ErrorKind::NotFound => match resume(root, task) {
                Ok(()) => {}
                Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
                Err(failure) => return Err(failure),
            },
            Err(failure) => return Err(failure),
        }
    }
    fs::remove_file(path)
}

pub(super) fn resume_groups(root: &Path, writer: &mut crate::journal::Writer) -> Vec<String> {
    let Ok(entries) = fs::read_dir(root.join("sessions")) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let id = name
                .to_str()?
                .strip_prefix(GROUP_PREFIX)?
                .strip_suffix(".json")?;
            let source = SessionId::parse_str(id).ok()?;
            if source.to_string() != id {
                return None;
            }
            complete_group(root, source, writer)
                .err()
                .map(|error| format!("会话 {source} 的成组删除待清理：{error}"))
        })
        .collect()
}

fn directory(path: &Path, parent: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(io::Error::other("待删除路径不是普通目录"));
    }
    let canonical = fs::canonicalize(path)?;
    if canonical.parent() != Some(parent) {
        return Err(io::Error::other("待删除目录超出所属范围"));
    }
    Ok(canonical)
}
fn regular(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(io::Error::other(format!(
            "拒绝清理非普通会话文件：{}",
            path.display()
        )));
    }
    Ok(())
}
fn temporary(name: &str) -> bool {
    name.starts_with(".tmp") && name.len() > ".tmp".len()
}
fn image(name: &str) -> bool {
    let Some((digest, extension)) = name.rsplit_once('.') else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
        && matches!(extension, "png" | "jpg")
}
fn files(path: &Path) -> io::Result<(Vec<PathBuf>, Option<PathBuf>)> {
    let mut files = Vec::new();
    let mut attachments = None;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| io::Error::other("会话中存在无法确认归属的文件名"))?;
        if name == "attachments" {
            let folder = directory(&entry.path(), path)?;
            for attachment in fs::read_dir(&folder)? {
                let attachment = attachment?;
                let name = attachment
                    .file_name()
                    .into_string()
                    .map_err(|_| io::Error::other("附件文件名无效"))?;
                if !image(&name)
                    && !crate::attachments::generated_file(&name)
                    && !crate::attachments::audio_file(&name)
                    && !crate::composer::input_image_file(&name)
                    && !temporary(&name)
                {
                    return Err(io::Error::other(format!("会话中存在未识别附件：{name}")));
                }
                regular(&attachment.path())?;
                files.push(attachment.path());
            }
            attachments = Some(folder);
        } else {
            let backup = name.starts_with("session-before-recovery-") && name.ends_with(".jsonl");
            if !matches!(
                name.as_str(),
                "session.jsonl" | "summary.json" | crate::projects::MEMBERSHIP_FILE
            ) && !backup
                && !temporary(&name)
            {
                return Err(io::Error::other(format!("会话中存在未识别文件：{name}")));
            }
            regular(&entry.path())?;
            if name != "session.jsonl" {
                files.push(entry.path());
            }
        }
    }
    Ok((files, attachments))
}
pub(super) fn resume(root: &Path, session: SessionId) -> io::Result<()> {
    let root = fs::canonicalize(root)?;
    let sessions = directory(&root.join("sessions"), &root)?;
    let path = directory(
        &sessions.join(format!("{PENDING_PREFIX}{session}")),
        &sessions,
    )?;
    let journal = path.join("session.jsonl");
    match fs::symlink_metadata(&journal) {
        Ok(_) => {
            let parsed = crate::reader::read(&path, session)?;
            if parsed.issue.is_some() || !parsed.summary.deleted {
                return Err(io::Error::other("待删除目录没有完整的已提交删除记录"));
            }
        }
        Err(failure)
            if failure.kind() == io::ErrorKind::NotFound
                && fs::read_dir(&path)?.next().is_none() =>
        {
            return fs::remove_dir(&path);
        }
        Err(failure) => return Err(failure),
    }
    let (files, attachments) = files(&path)?;
    for file in files {
        regular(&file)?;
        fs::remove_file(file)?;
    }
    if let Some(attachments) = attachments {
        directory(&attachments, &path)?;
        fs::remove_dir(attachments)?;
    }
    regular(&journal)?;
    fs::remove_file(journal)?;
    directory(&path, &sessions)?;
    fs::remove_dir(path)
}
pub(super) fn cleanup(root: &Path, session: SessionId) -> io::Result<()> {
    let source = crate::attachments::session_directory(root, session)?;
    let sessions = source
        .parent()
        .ok_or_else(|| io::Error::other("会话目录没有父路径"))?;
    let pending = sessions.join(format!("{PENDING_PREFIX}{session}"));
    match fs::symlink_metadata(&pending) {
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => return Err(io::Error::other("同一会话已有待删除目录，未覆盖")),
        Err(failure) => return Err(failure),
    }
    fs::rename(source, &pending)?;
    resume(root, session)
}
pub(super) fn delete(
    root: &Path,
    session: SessionId,
    writer: &mut crate::journal::Writer,
) -> io::Result<crate::DeleteCommit> {
    if !group_pending(root, session) {
        let tasks = linked(root, session)?;
        if tasks.is_empty() {
            return delete_one(root, session, writer);
        }
        for member in tasks.iter().copied().chain([session]) {
            let directory = crate::attachments::session_directory(root, member)?;
            let parsed = crate::reader::read(&directory, member)?;
            if let Some(issue) = parsed.issue {
                return Err(io::Error::other(issue.to_string()));
            }
            files(&directory)?;
        }
        write_group(
            root,
            &Group {
                source: session,
                tasks,
            },
        )?;
    }
    let cleanup_error = complete_group(root, session, writer).err();
    Ok(crate::DeleteCommit {
        session,
        cleanup_error,
    })
}

fn delete_one(
    root: &Path,
    session: SessionId,
    writer: &mut crate::journal::Writer,
) -> io::Result<crate::DeleteCommit> {
    match crate::attachments::session_directory(root, session) {
        Ok(directory) => {
            let parsed = crate::reader::read(&directory, session)?;
            if parsed
                .initial
                .as_ref()
                .is_some_and(|initial| !initial.settings.kind.is_user())
            {
                return Err(io::Error::other("此内部会话不能通过普通聊天删除"));
            }
            if let Some(issue) = parsed.issue {
                return Err(io::Error::other(issue.to_string()));
            }
            let summary_error = if parsed.summary.deleted {
                None
            } else {
                writer.delete(root, session)?.summary_failure
            };
            writer.reset(session);
            let cleanup_error = cleanup(root, session).err();
            if let Some(failure) = summary_error {
                tracing::error!(session_id = %session, error = %failure, cleanup_complete = cleanup_error.is_none(), "删除标记已提交，摘要更新失败，已按删除事实执行清理");
            }
            Ok(crate::DeleteCommit {
                session,
                cleanup_error,
            })
        }
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {
            let root = fs::canonicalize(root)?;
            let sessions = directory(&root.join("sessions"), &root)?;
            let pending = sessions.join(format!("{PENDING_PREFIX}{session}"));
            let _verified = directory(&pending, &sessions)?;
            Ok(crate::DeleteCommit {
                session,
                cleanup_error: resume(&root, session).err(),
            })
        }
        Err(failure) => Err(failure),
    }
}
