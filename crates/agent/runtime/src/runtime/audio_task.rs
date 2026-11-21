use agentelier_account::CredentialClient;
use agentelier_conversation::{
    AudioAttachment, AudioTranscription, RecordEvent, SessionId, TranscriptionOutcome,
    TranscriptionSource,
};
use agentelier_llm_model::CodexIdentity;
use agentelier_session_store::SessionStoreClient;
use std::io;
use tokio_util::sync::CancellationToken;

pub(super) enum Reply {
    Draft(io::Result<Option<AudioAttachment>>),
    Transcription(io::Result<AudioTranscription>),
}

pub(super) struct TranscriptionJob {
    pub(super) session: SessionId,
    pub(super) attachment: AudioAttachment,
    pub(super) source: TranscriptionSource,
    pub(super) owner: CodexIdentity,
    pub(super) cancel: CancellationToken,
}

impl TranscriptionJob {
    pub(super) async fn run(
        self,
        store: SessionStoreClient,
        credentials: CredentialClient,
    ) -> io::Result<AudioTranscription> {
        let Self {
            session,
            attachment,
            source,
            owner,
            cancel,
        } = self;
        save(
            &store,
            RecordEvent::TranscriptionStarted {
                session,
                source: source.clone(),
            },
        )
        .await?;
        let outcome = tokio::select! { biased;
            _ = cancel.cancelled() => TranscriptionOutcome::Cancelled,
            result = transcribe(&store, credentials, session, attachment, owner) => {
                match result {
                    Ok(text) => TranscriptionOutcome::Completed(text),
                    Err(failure) => TranscriptionOutcome::Failed(failure),
                }
            }
        };
        save(
            &store,
            RecordEvent::TranscriptionFinished {
                session,
                id: source.id,
                outcome: outcome.clone(),
            },
        )
        .await?;
        Ok(AudioTranscription { source, outcome })
    }
}

async fn transcribe(
    store: &SessionStoreClient,
    credentials: CredentialClient,
    session: SessionId,
    attachment: AudioAttachment,
    owner: CodexIdentity,
) -> Result<String, String> {
    let identity = credentials
        .execution()
        .await
        .map_err(|failure| failure.to_string())?;
    if identity.identity() != &owner {
        return Err("当前 Codex 凭据与附件所在会话的账号或工作区不一致".into());
    }
    let mime = attachment.format.mime();
    let bytes = store
        .read_audio(session, attachment)
        .await
        .map_err(|failure| format!("读取会话音频失败：{failure}"))?;
    agentelier_llm_model::transcribe_audio(&credentials, &identity, mime, bytes)
        .await
        .map_err(|failure| failure.to_string())
}

async fn save(store: &SessionStoreClient, event: RecordEvent) -> io::Result<()> {
    let session = event.session();
    let receipt = store.append(event).await?;
    if receipt.session != session {
        return Err(io::Error::other("转写记录保存回执不属于原会话"));
    }
    Ok(())
}
