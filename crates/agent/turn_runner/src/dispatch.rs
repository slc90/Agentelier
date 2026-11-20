use agentelier_conversation::ConversationFailure;
use agentelier_llm_model::FunctionCall;
use agentelier_tools::{CleanupReport, ExecutionContext, ToolAdmission, ToolCatalog, ToolResult};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

pub struct ToolBatch {
    pub calls: Vec<ToolJobRequest>,
    pub catalog: Arc<ToolCatalog>,
    pub context: Arc<ExecutionContext>,
}

pub struct ToolJobRequest {
    pub call: FunctionCall,
    pub admission: CancellationToken,
}

pub enum ToolEvent {
    Ready {
        call_id: String,
        begin: oneshot::Sender<bool>,
    },
    Finished {
        call_id: String,
        result: ToolResult,
    },
}

pub enum ToolCommand {
    Batch(ToolBatch),
    Accepted(String),
    ReapProcesses {
        reason: &'static str,
        reply: oneshot::Sender<CleanupReport>,
    },
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
    pub fn channel(
        admission: ToolAdmission,
    ) -> (
        Self,
        mpsc::Receiver<ToolCommand>,
        mpsc::UnboundedSender<ToolEvent>,
        mpsc::UnboundedReceiver<ToolEvent>,
    ) {
        let (sender, receiver) = mpsc::channel(1);
        let (events, incoming) = mpsc::unbounded_channel();
        (Self { sender, admission }, receiver, events, incoming)
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

    pub(crate) async fn reap_processes(
        &self,
        reason: &'static str,
    ) -> Result<CleanupReport, ConversationFailure> {
        let (reply, result) = oneshot::channel();
        self.send(ToolCommand::ReapProcesses { reason, reply })
            .await?;
        result
            .await
            .map_err(|_| ConversationFailure::new("进程资源回收未返回结果"))
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
