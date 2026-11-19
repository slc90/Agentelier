use agentelier_conversation::ConversationFailure;
use agentelier_llm_model::FunctionCall;
use agentelier_tools::{CleanupReport, ExecutionContext, ToolAdmission, ToolCatalog, ToolResult};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub struct ToolBatch {
    pub calls: Vec<FunctionCall>,
    pub catalog: Arc<ToolCatalog>,
    pub context: Arc<ExecutionContext>,
    pub admission: CancellationToken,
    pub events: mpsc::UnboundedSender<ToolEvent>,
}

pub enum ToolEvent {
    Ready {
        index: usize,
        begin: oneshot::Sender<bool>,
    },
    Finished {
        index: usize,
        result: ToolResult,
    },
}

pub enum ToolCommand {
    Batch(ToolBatch),
    Accepted(String),
    Cleanup {
        reason: &'static str,
        reply: oneshot::Sender<CleanupReport>,
    },
}

pub struct ToolDispatcher {
    sender: mpsc::Sender<ToolCommand>,
    admission: ToolAdmission,
}

impl ToolDispatcher {
    pub fn channel(admission: ToolAdmission) -> (Self, mpsc::Receiver<ToolCommand>) {
        let (sender, receiver) = mpsc::channel(1);
        (Self { sender, admission }, receiver)
    }

    pub(crate) fn is_blocked(&self) -> bool {
        self.admission.is_blocked()
    }

    pub(crate) async fn blocked(&self) {
        self.admission.blocked().await;
    }

    pub(crate) async fn send(&self, command: ToolCommand) -> Result<(), ConversationFailure> {
        self.sender
            .send(command)
            .await
            .map_err(|_| ConversationFailure::new("工具执行所有者已关闭"))
    }

    pub(crate) async fn cleanup(
        &self,
        reason: &'static str,
    ) -> Result<CleanupReport, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.send(ToolCommand::Cleanup { reason, reply }).await?;
        result
            .await
            .map_err(|_| ConversationFailure::new("工具资源回收未返回结果"))
    }
}
