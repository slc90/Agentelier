use crate::access::{RunnerEvent, TurnAccess};
use agentelier_conversation::{
    ConversationFailure, InputDecision, InputReceipt, RecordEvent, SessionId, SteerInput,
};
use tokio::sync::mpsc;

pub(crate) async fn receipt(
    progress: &mpsc::Sender<RunnerEvent>,
    id: u64,
    session: SessionId,
    decision: InputDecision,
) -> Result<(), ConversationFailure> {
    progress
        .send(RunnerEvent::Receipt(InputReceipt {
            id,
            session,
            decision,
        }))
        .await
        .map_err(|_| ConversationFailure::new("补充输入回执接收端已关闭"))
}
pub(crate) async fn accept(
    active: &TurnAccess,
    input: SteerInput,
    progress: &mpsc::Sender<RunnerEvent>,
) -> Result<(), ConversationFailure> {
    let id = input.id;
    let session = input.session;
    let accepted = active
        .call(move |mut turn| {
            turn.accept_steer(input)?;
            Ok(RecordEvent::SteerAccepted {
                state: turn.stored_state(),
            })
        })
        .await;
    let decision = match accepted {
        Ok(event) => {
            if let Err(failure) = active.record(event).await {
                receipt(
                    progress,
                    id,
                    session,
                    InputDecision::Rejected(format!("补充输入保存失败：{failure}")),
                )
                .await?;
                return Err(ConversationFailure::new(format!(
                    "补充输入保存失败：{failure}"
                )));
            }
            InputDecision::Accepted
        }
        Err(failure) => InputDecision::Rejected(failure.to_string()),
    };
    receipt(progress, id, session, decision).await
}
pub(crate) async fn drain(
    active: &TurnAccess,
    incoming: &mut mpsc::Receiver<SteerInput>,
    progress: &mpsc::Sender<RunnerEvent>,
) -> Result<(), ConversationFailure> {
    while let Ok(input) = incoming.try_recv() {
        accept(active, input, progress).await?;
    }
    Ok(())
}
pub(crate) async fn close(
    incoming: &mut mpsc::Receiver<SteerInput>,
    progress: &mpsc::Sender<RunnerEvent>,
) -> Result<(), ConversationFailure> {
    incoming.close();
    while let Ok(input) = incoming.try_recv() {
        receipt(
            progress,
            input.id,
            input.session,
            InputDecision::Rejected("任务已结束，补充输入未接纳".into()),
        )
        .await?;
    }
    Ok(())
}
