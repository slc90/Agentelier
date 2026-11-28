use agentelier_conversation::{
    AudioAttachment, ComposerContent, FileReference, FileScope, InputImage, SessionId,
};
use agentelier_session_store::SessionStoreClient;
use agentelier_tools::ImagePayload;
use std::{future::Future, io, path::PathBuf, pin::Pin, sync::Arc};

pub enum ComposerImageSource {
    Local(PathBuf),
    Project {
        reference: FileReference,
        scope: FileScope,
    },
    Clipboard(image::RgbaImage),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ComposerAssetOwner {
    Draft(SessionId),
    Session(SessionId),
}

pub enum ComposerAssetRequest {
    StageImages {
        draft: SessionId,
        sources: Vec<ComposerImageSource>,
    },
    StageAudio {
        draft: SessionId,
        paths: Vec<PathBuf>,
    },
    Audio {
        owner: ComposerAssetOwner,
        audio: AudioAttachment,
    },
    Image {
        owner: ComposerAssetOwner,
        image: InputImage,
    },
}

pub enum ComposerAssetReply {
    Staged(Vec<io::Result<ComposerContent>>),
    Image(Arc<ImagePayload>),
    Audio(Arc<[u8]>),
}

pub type ComposerAssetFuture = Pin<Box<dyn Future<Output = io::Result<ComposerAssetReply>> + Send>>;

pub(super) async fn request(
    store: SessionStoreClient,
    request: ComposerAssetRequest,
) -> io::Result<ComposerAssetReply> {
    match request {
        ComposerAssetRequest::StageImages { draft, sources } => {
            let mut results = Vec::with_capacity(sources.len());
            for source in sources {
                let result = async {
                    let path = match source {
                        ComposerImageSource::Local(path) => path,
                        ComposerImageSource::Project { reference, scope } => {
                            tokio::task::spawn_blocking(move || reference.verify(&scope))
                                .await
                                .map_err(io::Error::other)??
                        }
                        ComposerImageSource::Clipboard(pixels) => {
                            return store.stage_clipboard_image(draft, pixels).await;
                        }
                    };
                    store.stage_input(draft, path, true).await
                }
                .await;
                results.push(result);
            }
            Ok(ComposerAssetReply::Staged(results))
        }
        ComposerAssetRequest::StageAudio { draft, paths } => {
            let mut results = Vec::with_capacity(paths.len());
            for path in paths {
                results.push(store.stage_input(draft, path, false).await);
            }
            Ok(ComposerAssetReply::Staged(results))
        }
        ComposerAssetRequest::Audio { owner, audio } => {
            let bytes = match owner {
                ComposerAssetOwner::Draft(id) => store.draft_input_audio(id, audio).await?,
                ComposerAssetOwner::Session(id) => store.read_audio(id, audio).await?,
            };
            Ok(ComposerAssetReply::Audio(bytes.into()))
        }
        ComposerAssetRequest::Image { owner, image } => {
            let (id, draft) = match owner {
                ComposerAssetOwner::Draft(id) => (id, true),
                ComposerAssetOwner::Session(id) => (id, false),
            };
            store
                .input_image(id, image, draft)
                .await
                .map(ComposerAssetReply::Image)
        }
    }
}
