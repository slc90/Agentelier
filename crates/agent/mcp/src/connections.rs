use crate::{Server, ServerId, Transport, stdio};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::{
    sync::mpsc,
    task::{Id, JoinSet},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone)]
pub struct ServerCapabilities {
    pub version: String,
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
}

#[derive(Clone)]
pub enum ConnectionState {
    Starting,
    Negotiating,
    Ready(ServerCapabilities),
    Closing,
    Closed,
    Failed {
        stage: &'static str,
        reason: &'static str,
    },
}

#[derive(Clone)]
pub struct ConnectionStatus {
    pub server: ServerId,
    pub state: ConnectionState,
}

pub(crate) struct Failure {
    pub(crate) stage: &'static str,
    pub(crate) reason: &'static str,
}

struct Connection {
    transport: Transport,
    cancel: CancellationToken,
    task: Option<Id>,
    state: ConnectionState,
}

pub struct Connections {
    configured: BTreeMap<ServerId, Transport>,
    known: BTreeSet<ServerId>,
    active: BTreeMap<ServerId, Connection>,
    tasks: JoinSet<Result<(), Failure>>,
    progress: mpsc::UnboundedReceiver<(Id, ConnectionState)>,
    publish: mpsc::UnboundedSender<(Id, ConnectionState)>,
}

impl Default for Connections {
    fn default() -> Self {
        let (publish, progress) = mpsc::unbounded_channel();
        Self {
            configured: BTreeMap::new(),
            known: BTreeSet::new(),
            active: BTreeMap::new(),
            tasks: JoinSet::new(),
            progress,
            publish,
        }
    }
}

impl Connections {
    pub fn configure(&mut self, servers: &[Server]) {
        self.known = servers.iter().map(|server| server.id).collect();
        self.configured = servers
            .iter()
            .filter(|server| {
                server.settings.enabled
                    && matches!(server.settings.transport, Transport::Stdio { .. })
            })
            .map(|server| (server.id, server.settings.transport.clone()))
            .collect();
        self.reconcile();
    }

    fn reconcile(&mut self) {
        self.active.retain(|id, entry| {
            if self.configured.get(id) == Some(&entry.transport) && !entry.cancel.is_cancelled() {
                return true;
            }
            if entry.task.is_some() {
                entry.state = ConnectionState::Closing;
                entry.cancel.cancel();
                return true;
            }
            self.known.contains(id)
                && (matches!(
                    entry.state,
                    ConnectionState::Failed {
                        stage: "回收", ..
                    }
                ) || (!self.configured.contains_key(id)
                    && matches!(entry.state, ConnectionState::Closed)))
        });
        for (id, transport) in &self.configured {
            if self.active.contains_key(id) {
                continue;
            }
            let transport = transport.clone();
            let task_transport = transport.clone();
            let cancel = CancellationToken::new();
            let stop = cancel.clone();
            let publish = self.publish.clone();
            let server = *id;
            let task = self
                .tasks
                .spawn(async move {
                    let task = tokio::task::id();
                    debug!(server_id = %server, "开始启动 MCP 服务");
                    stdio::run(task_transport, stop, |state| {
                        if publish.send((task, state)).is_err() {
                            debug!(server_id = %server, "MCP 状态接收端已关闭");
                        }
                    })
                    .await
                })
                .id();
            self.active.insert(
                *id,
                Connection {
                    transport,
                    cancel,
                    task: Some(task),
                    state: ConnectionState::Starting,
                },
            );
        }
    }

    pub fn statuses(&self) -> Vec<ConnectionStatus> {
        self.active
            .iter()
            .map(|(server, connection)| ConnectionStatus {
                server: *server,
                state: connection.state.clone(),
            })
            .collect()
    }

    pub async fn changed(&mut self) -> Vec<ConnectionStatus> {
        tokio::select! {
            Some((task, state)) = self.progress.recv() => {
                if let Some((server, entry)) = self.active.iter_mut().find(|(_, entry)| entry.task == Some(task))
                    && !entry.cancel.is_cancelled()
                {
                    if let ConnectionState::Ready(capabilities) = &state {
                        debug!(server_id = %server, protocol = %capabilities.version, "MCP 协议已就绪");
                    }
                    entry.state = state;
                }
            }
            Some(result) = self.tasks.join_next_with_id(), if !self.tasks.is_empty() => {
                let (task, result) = match result {
                    Ok(result) => result,
                    Err(error) => (error.id(), Err(Failure { stage: "回收", reason: "MCP 连接任务异常结束" })),
                };
                if let Some((server, entry)) = self.active.iter_mut().find(|(_, entry)| entry.task == Some(task)) {
                    entry.task = None;
                    entry.state = match result {
                        Ok(()) => ConnectionState::Closed,
                        Err(failure) => {
                            error!(server_id = %server, stage = failure.stage, reason = failure.reason, "MCP 连接失败");
                            ConnectionState::Failed { stage: failure.stage, reason: failure.reason }
                        }
                    };
                }
                self.reconcile();
            }
        }
        self.statuses()
    }

    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        for entry in self.active.values() {
            entry.cancel.cancel();
        }
        let mut failure = None;
        while let Some(result) = self.tasks.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) if error.stage != "回收" => {}
                Ok(Err(error)) => {
                    failure = Some(std::io::Error::other(error.reason));
                }
                Err(_) => {
                    failure = Some(std::io::Error::other("MCP 连接任务异常结束"));
                }
            }
        }
        self.active.clear();
        failure.map_or(Ok(()), Err)
    }
}

pub(crate) async fn serve(
    transport: impl rmcp::transport::Transport<rmcp::RoleClient> + 'static,
    stop: &CancellationToken,
    publish: impl Fn(ConnectionState),
) -> Result<(), Failure> {
    use rmcp::model::{ClientCapabilities, ClientConfig, Implementation, ProtocolVersion};
    use rmcp::service::serve_client_with_lifecycle_and_ct;
    let client = ClientConfig::new(
        ClientCapabilities::default(),
        Implementation::new("Agentelier", env!("CARGO_PKG_VERSION")),
    );
    let lifecycle = rmcp::ClientLifecycleMode::Auto {
        preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        legacy_version: Some(ProtocolVersion::V_2025_11_25),
    };
    publish(ConnectionState::Negotiating);
    let client = tokio::select! {
        biased;
        _ = stop.cancelled() => return Ok(()),
        result = tokio::time::timeout(
            NEGOTIATION_TIMEOUT,
            serve_client_with_lifecycle_and_ct(client, transport, lifecycle, CancellationToken::new()),
        ) => match result {
            Ok(Ok(client)) => client,
            Ok(Err(error)) => return Err(negotiation_failure(&error)),
            Err(_) => return Err(Failure { stage: "协商", reason: "服务器初始化超时" }),
        }
    };
    let Some(info) = client.peer_info() else {
        let mut client = client;
        close(&mut client).await?;
        return Err(Failure {
            stage: "协商",
            reason: "服务器没有返回有效的能力信息",
        });
    };
    let cancel = client.cancellation_token();
    publish(ConnectionState::Ready(ServerCapabilities {
        version: info.protocol_version.to_string(),
        tools: info.capabilities.tools.is_some(),
        resources: info.capabilities.resources.is_some(),
        prompts: info.capabilities.prompts.is_some(),
    }));
    let waiting = client.waiting();
    tokio::pin!(waiting);
    tokio::select! {
        biased;
        _ = stop.cancelled() => {
            cancel.cancel();
            match tokio::time::timeout(CLOSE_TIMEOUT, waiting).await {
                Ok(Ok(_)) => Ok(()),
                _ => Err(Failure { stage: "回收", reason: "无法确认 MCP 协议连接关闭" }),
            }
        }
        _ = &mut waiting => Err(Failure { stage: "连接", reason: "MCP 服务器已断开" }),
    }
}

async fn close(
    client: &mut rmcp::service::RunningService<rmcp::RoleClient, rmcp::model::ClientConfig>,
) -> Result<(), Failure> {
    match client.close_with_timeout(CLOSE_TIMEOUT).await {
        Ok(Some(_)) => Ok(()),
        _ => Err(Failure {
            stage: "回收",
            reason: "无法确认 MCP 协议连接关闭",
        }),
    }
}

fn negotiation_failure(error: &rmcp::service::ClientInitializeError) -> Failure {
    use rmcp::service::ClientInitializeError;
    let reason = match error {
        ClientInitializeError::NoCompatibleProtocolVersion { .. } => "双方没有兼容的 MCP 协议版本",
        ClientInitializeError::ConnectionClosed(_)
        | ClientInitializeError::ExpectedInitResponse(None) => {
            "服务器在协商完成前退出或关闭了协议流"
        }
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            return negotiation_failure(fallback);
        }
        _ => "服务器发送了无效的初始化响应",
    };
    Failure {
        stage: "协商",
        reason,
    }
}
