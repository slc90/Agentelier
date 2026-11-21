use agentelier_conversation::SessionId;
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::{ImageDescriptor, OriginalImage};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const MAX_PREVIEW_EDGE: u32 = 512;

pub enum ImageAttachmentAction {
    Preview,
    Export(PathBuf),
}

pub enum ImageAttachmentReply {
    Preview {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Exported(PathBuf),
}

pub(super) async fn execute(
    store: SessionStoreClient,
    root: PathBuf,
    session: SessionId,
    image: ImageDescriptor,
    action: ImageAttachmentAction,
) -> io::Result<ImageAttachmentReply> {
    let original = store.read_generated(session, image).await?;
    tokio::task::spawn_blocking(move || match action {
        ImageAttachmentAction::Preview => preview(&original),
        ImageAttachmentAction::Export(path) => export(&root, &original, &path),
    })
    .await
    .map_err(io::Error::other)?
}

fn preview(original: &OriginalImage) -> io::Result<ImageAttachmentReply> {
    let decoded = image::load_from_memory(original.bytes()).map_err(io::Error::other)?;
    let pixels = decoded
        .thumbnail(MAX_PREVIEW_EDGE, MAX_PREVIEW_EDGE)
        .into_rgba8();
    Ok(ImageAttachmentReply::Preview {
        width: pixels.width(),
        height: pixels.height(),
        rgba: pixels.into_raw(),
    })
}

fn export(root: &Path, original: &OriginalImage, path: &Path) -> io::Result<ImageAttachmentReply> {
    let parent = path
        .parent()
        .filter(|_| path.is_absolute())
        .ok_or_else(|| io::Error::other("另存为需要绝对文件路径"))?
        .canonicalize()?;
    if parent.starts_with(root.canonicalize()?) {
        return Err(io::Error::other("请选择应用数据目录之外的位置"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("缺少导出文件名"))?;
    let destination = parent.join(name);
    let mut temporary = tempfile::NamedTempFile::new_in(&parent)?;
    temporary.write_all(original.bytes())?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(destination)
        .map_err(|failure| failure.error)?;
    Ok(ImageAttachmentReply::Exported(path.to_owned()))
}
