use agentelier_account::CredentialClient;
use agentelier_conversation::{ActiveTurn, ConversationFailure, SessionView};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MODEL_PROGRESS_CAPACITY: usize = 32;

pub async fn run(
    mut active: ActiveTurn,
    credentials: CredentialClient,
    cancel: CancellationToken,
    progress: mpsc::Sender<SessionView>,
) -> Result<ActiveTurn, ConversationFailure> {
    let (sender, mut events) = mpsc::channel(MODEL_PROGRESS_CAPACITY);
    let request = active.request()?;
    let outcome = {
        let generation = agentelier_llm_model::generate(credentials, request, sender);
        tokio::pin!(generation);
        loop {
            tokio::select! { biased;
                result = &mut generation => break Some(result),
                _ = cancel.cancelled() => break None,
                event = events.recv() => {
                    if let Some(event) = event {
                        let view = active.update(event)?;
                        tokio::select! { biased;
                            _ = cancel.cancelled() => break None,
                            result = progress.send(view) => {
                                if result.is_err() {
                                    return Err(ConversationFailure::new("任务进度接收端已关闭"));
                                }
                            }
                        }
                    }
                }
            }
        }
    };
    while let Ok(event) = events.try_recv() {
        active.update(event)?;
    }
    active.finish(outcome)?;
    Ok(active)
}
