use agentelier_conversation::{SessionId, StoredImage};
use agentelier_tools::{ImagePayload, MAX_ENCODED_IMAGE_BYTES, MAX_TOOL_BODY_BYTES};
use std::io::{Read, Write};
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fs, io};
use tempfile::NamedTempFile;

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

fn directory(path: &Path, parent: &Path) -> io::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("会话附件路径不是普通目录或包含重解析点"));
    }
    let canonical = fs::canonicalize(path)?;
    if canonical.parent() != Some(parent) {
        return Err(io::Error::other("会话附件目录超出所属范围"));
    }
    Ok(canonical)
}

pub(super) fn session_directory(root: &Path, session: SessionId) -> io::Result<PathBuf> {
    if session.is_nil() {
        return Err(io::Error::other("附件缺少所属会话"));
    }
    let root = fs::canonicalize(root)?;
    let sessions = directory(&root.join("sessions"), &root)?;
    let session = directory(&sessions.join(session.to_string()), &sessions)?;
    let journal = fs::symlink_metadata(session.join("session.jsonl"))?;
    if !journal.is_file() || journal.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::other("附件所属会话缺少普通存档文件"));
    }
    Ok(session)
}

fn attachment_directory(root: &Path, session: SessionId, create: bool) -> io::Result<PathBuf> {
    let session = session_directory(root, session)?;
    let path = session.join("attachments");
    if create {
        match fs::create_dir(&path) {
            Ok(()) => {}
            Err(failure) if failure.kind() == io::ErrorKind::AlreadyExists => {}
            Err(failure) => return Err(failure),
        }
    }
    directory(&path, &session)
}

fn filename(reference: &StoredImage) -> io::Result<String> {
    reference.image.validate()?;
    if reference.summary.len() > MAX_TOOL_BODY_BYTES {
        return Err(io::Error::other("附件说明超过工具文字限额"));
    }
    Ok(format!(
        "{}.{}",
        reference.image.digest,
        reference.image.extension()?
    ))
}

fn read_file(path: &Path, reference: &StoredImage) -> io::Result<Arc<ImagePayload>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() != reference.image.encoded_bytes as u64
    {
        return Err(io::Error::other("附件不是普通文件或编码长度不符"));
    }
    let mut bytes = Vec::new();
    file.take(MAX_ENCODED_IMAGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    ImagePayload::from_encoded(&reference.image, bytes).map(Arc::new)
}

pub(super) fn read(root: &Path, reference: &StoredImage) -> io::Result<Arc<ImagePayload>> {
    let name = filename(reference)?;
    let directory = attachment_directory(root, reference.session, false)?;
    read_file(&directory.join(name), reference)
}

pub(super) fn save(root: &Path, reference: &StoredImage, image: &ImagePayload) -> io::Result<()> {
    let name = filename(reference)?;
    if image.descriptor() != reference.image {
        return Err(io::Error::other("图片载荷与附件引用不一致"));
    }
    let directory = attachment_directory(root, reference.session, true)?;
    let path = directory.join(name);
    match fs::symlink_metadata(&path) {
        Ok(_) => {
            read_file(&path, reference)?;
            return Ok(());
        }
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure),
    }
    let mut temporary = NamedTempFile::new_in(&directory)?;
    let outcome = (|| {
        temporary.write_all(image.bytes())?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        read_file(temporary.path(), reference)?;
        Ok::<(), io::Error>(())
    })();
    if let Err(failure) = outcome {
        let path = temporary.path().to_path_buf();
        return match temporary.close() {
            Ok(()) => Err(failure),
            Err(cleanup) => Err(io::Error::other(format!(
                "附件未提交：{failure}；临时文件清理失败 {}：{cleanup}",
                path.display()
            ))),
        };
    }
    match temporary.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(failure) => {
            let original = failure.error;
            let temporary_path = failure.file.path().to_path_buf();
            failure.file.close().map_err(|cleanup| {
                io::Error::other(format!(
                    "附件提交失败：{original}；临时文件清理失败 {}：{cleanup}",
                    temporary_path.display()
                ))
            })?;
            if original.kind() == io::ErrorKind::AlreadyExists {
                read_file(&path, reference)?;
                Ok(())
            } else {
                Err(original)
            }
        }
    }
}
