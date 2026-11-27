use crate::attachments;
use agentelier_conversation::{
    AudioAttachment, ComposerContent, ComposerMessage, InputImage, RecordEvent, SessionId,
};
use agentelier_tools::{
    ImagePayload, MAX_ORIGINAL_IMAGE_BYTES, MAX_PROMPT_IMAGE_BYTES, OriginalImage,
};
use std::{
    fs,
    io::{self, Read, Write},
    path::Path,
    sync::Arc,
};
use tempfile::NamedTempFile;

#[derive(Default)]
pub(super) struct Drafts(Option<tempfile::TempDir>);

impl Drafts {
    fn directory(&self, owner: SessionId) -> io::Result<std::path::PathBuf> {
        if owner.is_nil() {
            return Err(io::Error::other("附件草稿缺少身份"));
        }
        self.0
            .as_ref()
            .map(|root| root.path().join(owner.to_string()))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "没有已暂存的附件"))
    }

    pub(super) fn stage(
        &mut self,
        root: &Path,
        owner: SessionId,
        source: Imported,
    ) -> io::Result<ComposerContent> {
        if self.0.is_none() {
            self.0 = Some(
                tempfile::Builder::new()
                    .prefix(".composer-drafts-")
                    .tempdir_in(root)?,
            );
        }
        let directory = self.directory(owner)?;
        fs::create_dir_all(&directory)?;
        let (attachment, bytes, id) = match &source {
            Imported::Image(image, payload) => (
                ComposerContent::Image {
                    image: image.clone(),
                },
                payload.bytes(),
                image.id,
            ),
            Imported::Audio(imported) => (
                ComposerContent::Audio {
                    audio: imported.attachment.clone(),
                },
                imported.bytes.as_ref(),
                imported.attachment.id,
            ),
        };
        let mut temporary = NamedTempFile::new_in(&directory)?;
        temporary.write_all(bytes)?;
        temporary.as_file().sync_all()?;
        temporary
            .persist_noclobber(directory.join(id.to_string()))
            .map_err(|failure| failure.error)?;
        Ok(attachment)
    }

    pub(super) fn read_image(
        &self,
        owner: SessionId,
        image: &InputImage,
    ) -> io::Result<Arc<ImagePayload>> {
        read_image_file(&self.directory(owner)?.join(image.id.to_string()), image)
    }

    pub(super) fn read_audio(
        &self,
        owner: SessionId,
        audio: &AudioAttachment,
    ) -> io::Result<Vec<u8>> {
        attachments::read_audio_file(&self.directory(owner)?.join(audio.id.to_string()), audio)
    }

    pub(super) fn bind(&self, session: &Path, event: &RecordEvent) -> io::Result<()> {
        let Some(message) = admitted_input(event) else {
            return Ok(());
        };
        if message.images().next().is_none() && message.audio().next().is_none() {
            return Ok(());
        }
        let directory = attachments::local_attachment_directory(session, true)?;
        for image in message.images() {
            let path = directory.join(image_filename(image)?);
            if let Some(owner) = message.draft_owner().filter(|_| !path.exists()) {
                let payload = self.read_image(owner, image)?;
                attachments::write_encoded(&path, payload.bytes(), |path| {
                    read_image_file(path, image).map(drop)
                })?;
            } else {
                read_image_file(&path, image)?;
            }
        }
        for audio in message.audio() {
            let path = directory.join(attachments::audio_filename(audio)?);
            if let Some(owner) = message.draft_owner().filter(|_| !path.exists()) {
                let bytes = self.read_audio(owner, audio)?;
                attachments::write_encoded(&path, &bytes, |path| {
                    attachments::read_audio_file(path, audio).map(drop)
                })?;
            } else {
                attachments::read_audio_file(&path, audio)?;
            }
        }
        Ok(())
    }
}

pub(super) enum Imported {
    Image(InputImage, ImagePayload),
    Audio(crate::audio::ImportedAudio),
}

impl Imported {
    pub(super) fn image(path: &Path) -> io::Result<Self> {
        let name: String = path
            .file_name()
            .ok_or_else(|| io::Error::other("请选择图片文件"))?
            .to_string_lossy()
            .chars()
            .filter(|c| !c.is_control())
            .collect();
        let file = fs::File::open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() > MAX_ORIGINAL_IMAGE_BYTES as u64 {
            return Err(io::Error::other("所选图片不是普通文件或超过图片输入限额"));
        }
        let mut bytes = Vec::new();
        (&file)
            .take(MAX_ORIGINAL_IMAGE_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        let after = file.metadata()?;
        if metadata.len() != bytes.len() as u64
            || after.len() != metadata.len()
            || after.modified()? != metadata.modified()?
        {
            return Err(io::Error::other("图片文件在读取期间变化"));
        }
        let payload = OriginalImage::decode(bytes)?.into_model_image()?;
        let image = InputImage {
            id: SessionId::new_v4(),
            name,
            image: payload.descriptor(),
        };
        Ok(Self::Image(image, payload))
    }
}

pub(super) fn admitted_input(event: &RecordEvent) -> Option<&ComposerMessage> {
    match event {
        RecordEvent::Accepted { state } => state.turn.as_ref().map(|turn| &turn.submission),
        RecordEvent::QueueChanged {
            state,
            decision: agentelier_conversation::QueueDecision::Enqueued,
        } => state.queue.last().map(|input| &input.submission),
        RecordEvent::AgentTurn { event, .. } => admitted_input(event),
        _ => None,
    }
}

pub(super) fn input_image_file(name: &str) -> bool {
    name.strip_prefix("input-")
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(id, extension)| {
            SessionId::parse_str(id).is_ok_and(|uuid| uuid.to_string() == id)
                && matches!(extension, "png" | "jpg")
        })
}

fn image_filename(image: &InputImage) -> io::Result<String> {
    image.image.validate()?;
    if image.id.is_nil() {
        return Err(io::Error::other("图片附件身份无效"));
    }
    Ok(format!("input-{}.{}", image.id, image.image.extension()?))
}

fn read_image_file(path: &Path, image: &InputImage) -> io::Result<Arc<ImagePayload>> {
    let bytes = attachments::read_encoded(path, image.image.encoded_bytes, MAX_PROMPT_IMAGE_BYTES)?;
    ImagePayload::from_encoded(&image.image, bytes).map(Arc::new)
}

pub(super) fn read_image(
    root: &Path,
    session: SessionId,
    image: &InputImage,
) -> io::Result<Arc<ImagePayload>> {
    let directory = attachments::session_directory(root, session)?;
    let directory = attachments::local_attachment_directory(&directory, false)?;
    read_image_file(&directory.join(image_filename(image)?), image)
}
