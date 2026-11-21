use agentelier_conversation::{SessionId, StoredImage};
use agentelier_tools::{
    ImageDescriptor, ImagePayload, MAX_ENCODED_IMAGE_BYTES, MAX_ORIGINAL_IMAGE_BYTES,
    MAX_SESSION_ORIGINAL_IMAGE_BYTES, MAX_TOOL_BODY_BYTES, OriginalImage,
};
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
    let bytes = read_encoded(path, reference.image.encoded_bytes, MAX_ENCODED_IMAGE_BYTES)?;
    ImagePayload::from_encoded(&reference.image, bytes).map(Arc::new)
}

fn read_encoded(path: &Path, expected: usize, limit: usize) -> io::Result<Vec<u8>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || metadata.len() != expected as u64
    {
        return Err(io::Error::other("附件不是普通文件或编码长度不符"));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    Ok(bytes)
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
    write_encoded(&path, image.bytes(), |path| {
        read_file(path, reference).map(drop)
    })
}

fn write_encoded(
    path: &Path,
    bytes: &[u8],
    verify: impl Fn(&Path) -> io::Result<()>,
) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => return verify(path),
        Err(failure) if failure.kind() == io::ErrorKind::NotFound => {}
        Err(failure) => return Err(failure),
    }
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other("缺少附件目录"))?;
    let mut temporary = NamedTempFile::new_in(directory)?;
    let outcome = (|| {
        temporary.write_all(bytes)?;
        temporary.as_file_mut().flush()?;
        temporary.as_file().sync_all()?;
        verify(temporary.path())?;
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
    match temporary.persist_noclobber(path) {
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
                verify(path)
            } else {
                Err(original)
            }
        }
    }
}

const GENERATED_PREFIX: &str = "generated-";

fn generated_filename(image: &ImageDescriptor) -> io::Result<String> {
    image.validate_original()?;
    Ok(format!(
        "{GENERATED_PREFIX}{}.{}",
        image.digest,
        image.extension()?
    ))
}

pub(super) fn generated_file(name: &str) -> bool {
    let Some((digest, extension)) = name
        .strip_prefix(GENERATED_PREFIX)
        .and_then(|name| name.rsplit_once('.'))
    else {
        return false;
    };
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        && matches!(extension, "png" | "jpg")
}

fn read_original(path: &Path, image: &ImageDescriptor) -> io::Result<OriginalImage> {
    let bytes = read_encoded(path, image.encoded_bytes, MAX_ORIGINAL_IMAGE_BYTES)?;
    let original = OriginalImage::decode(bytes)?;
    if original.descriptor() != image {
        return Err(io::Error::other("已保存的生成图片不符合内容引用"));
    }
    Ok(original)
}

pub(super) fn validate_generated(
    root: &Path,
    session: SessionId,
    image: &ImageDescriptor,
) -> io::Result<()> {
    read_generated(root, session, image).map(drop)
}

pub(super) fn read_generated(
    root: &Path,
    session: SessionId,
    image: &ImageDescriptor,
) -> io::Result<OriginalImage> {
    let name = generated_filename(image)?;
    let directory = attachment_directory(root, session, false)?;
    read_original(&directory.join(name), image)
}

pub(super) fn save_generated(
    root: &Path,
    session: SessionId,
    original: &OriginalImage,
) -> io::Result<PathBuf> {
    let image = original.descriptor();
    let name = generated_filename(image)?;
    let directory = attachment_directory(root, session, true)?;
    let path = directory.join(&name);
    let mut used = 0_u64;
    for entry in fs::read_dir(&directory)? {
        let entry = entry?;
        let filename = entry.file_name();
        if filename != name.as_str() && filename.to_str().is_some_and(generated_file) {
            used = used.saturating_add(entry.metadata()?.len());
        }
    }
    if used.saturating_add(image.encoded_bytes as u64) > MAX_SESSION_ORIGINAL_IMAGE_BYTES {
        return Err(io::Error::other("生成原图超过会话 256 MiB 累计限额"));
    }
    write_encoded(&path, original.bytes(), |path| {
        read_original(path, image).map(drop)
    })?;
    Ok(root
        .join("sessions")
        .join(session.to_string())
        .join("attachments")
        .join(name))
}
