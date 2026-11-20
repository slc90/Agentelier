use agentelier_conversation::SessionId;
use std::fs;
use std::io;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

const REPARSE_POINT: u32 = 0x0000_0400;
pub(super) const PENDING_PREFIX: &str = ".deleting-";

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
                if !image(&name) && !crate::attachments::generated_file(&name) && !temporary(&name)
                {
                    return Err(io::Error::other(format!("会话中存在未识别附件：{name}")));
                }
                regular(&attachment.path())?;
                files.push(attachment.path());
            }
            attachments = Some(folder);
        } else {
            let backup = name.starts_with("session-before-recovery-") && name.ends_with(".jsonl");
            if !matches!(name.as_str(), "session.jsonl" | "summary.json")
                && !backup
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
    match crate::attachments::session_directory(root, session) {
        Ok(directory) => {
            let parsed = crate::reader::read(&directory, session)?;
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
