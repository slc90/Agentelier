use agentelier_account_bridge::AccountEventKind;
use agentelier_bridge_connection::ecs_access::{EcsAccess, EcsRequest};
use agentelier_bridge_connection::{BridgeEventKind, BridgeFailure, Connection, DataEnvironment};
use agentelier_conversation_bridge::SessionId;
use agentelier_llm_model_bridge::ModelEventKind;
use bevy::prelude::*;
use bevy::winit::{EventLoopProxyWrapper, WinitUserEvent};
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex, MutexGuard};
use tracing::debug;

const MAX_EVENTS_PER_UPDATE: usize = 32;

#[derive(Resource, Clone, Default)]
pub struct AgentHost(Arc<Mutex<HostState>>);

#[derive(Default)]
struct HostState {
    connection: Option<Connection>,
    ecs_requests: Option<Receiver<EcsRequest>>,
    notify: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    stopping: bool,
    failed: bool,
    model_busy: bool,
}

#[derive(Resource, Default)]
pub struct HostActivity {
    pub model_busy: bool,
}

#[derive(Message)]
pub struct AccountEvent {
    pub operation_id: u64,
    pub kind: AccountEventKind,
}

#[derive(Message)]
pub struct ModelEvent {
    pub operation_id: u64,
    pub kind: ModelEventKind,
}

#[derive(Message)]
pub struct ConversationEvent {
    pub operation_id: u64,
    pub kind: agentelier_conversation_bridge::ConversationEvent,
}

#[derive(Message)]
pub struct GlobalTasksEvent(pub agentelier_conversation_bridge::GlobalTasks);

#[derive(Message)]
pub struct GlobalTaskNotificationEvent(pub agentelier_conversation_bridge::TaskNotification);

#[derive(Message)]
pub struct MemoryEvent {
    pub operation_id: u64,
    pub progress: bool,
    pub result: std::result::Result<agentelier_bridge_connection::MemoryReply, String>,
}

#[derive(Message)]
pub struct MemoryWorkEvent(pub agentelier_bridge_connection::MemoryWorkState);

#[derive(Message)]
pub struct ImageAttachmentEvent {
    pub operation_id: u64,
    pub result: std::result::Result<agentelier_bridge_connection::ImageAttachmentReply, String>,
}

#[derive(Message)]
pub struct McpConfigEvent {
    pub operation_id: u64,
    pub result: std::result::Result<agentelier_bridge_connection::ConfigSnapshot, String>,
}

#[derive(Message)]
pub struct SkillsEvent {
    pub operation_id: u64,
    pub result: std::result::Result<agentelier_bridge_connection::SkillSnapshot, String>,
}

#[derive(Message)]
pub struct McpMaterialEvent {
    pub operation_id: u64,
    pub result: std::result::Result<agentelier_bridge_connection::MaterialReply, String>,
}

#[derive(Message)]
pub struct McpStatusEvent(pub Vec<agentelier_bridge_connection::ConnectionStatus>);

#[derive(Message, Clone)]
pub struct HostFailed {
    pub reason: String,
}

#[derive(SystemSet, Debug, Hash, PartialEq, Eq, Clone)]
pub struct ReceiveAgentEvents;

#[derive(Default)]
pub struct AgentHostPlugin {
    host: AgentHost,
    ecs_dispatch: crate::ecs_access::dispatch::Registry,
    ecs_scenes: crate::ecs_access::scenes::Registry,
}

impl AgentHostPlugin {
    pub fn host(&self) -> AgentHost {
        self.host.clone()
    }

    pub fn with_ecs_unit_event<E: Event<Trigger<'static>: Default>>(
        mut self,
        name: &'static str,
        description: &'static str,
        event: fn() -> E,
    ) -> Result<Self> {
        self.ecs_dispatch
            .register_unit_event(name, description, event)?;
        Ok(self)
    }

    pub fn with_ecs_scene<P: serde::de::DeserializeOwned + 'static, S: Scene>(
        mut self,
        name: &'static str,
        description: &'static str,
        parameters: &'static [(&'static str, &'static str)],
        build: fn(P) -> Result<S>,
    ) -> Result<Self> {
        self.ecs_scenes
            .register(name, description, parameters, build)?;
        Ok(self)
    }
}

impl Plugin for AgentHostPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.host.clone())
            .insert_resource(self.ecs_dispatch.clone())
            .insert_resource(self.ecs_scenes.clone())
            .init_resource::<HostActivity>()
            .add_message::<AccountEvent>()
            .add_message::<ModelEvent>()
            .add_message::<ConversationEvent>()
            .add_message::<GlobalTasksEvent>()
            .add_message::<GlobalTaskNotificationEvent>()
            .add_message::<HostFailed>()
            .add_message::<McpConfigEvent>()
            .add_message::<SkillsEvent>()
            .add_message::<MemoryEvent>()
            .add_message::<MemoryWorkEvent>()
            .add_message::<ImageAttachmentEvent>()
            .add_message::<McpStatusEvent>()
            .add_message::<McpMaterialEvent>()
            .add_systems(
                Update,
                (receive, crate::ecs_access::process)
                    .chain()
                    .in_set(ReceiveAgentEvents),
            );
    }
}

impl AgentHost {
    fn state(&self) -> Result<MutexGuard<'_, HostState>> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("执行宿主状态锁已损坏").into())
    }

    pub fn prepare(&self, environment: DataEnvironment, proxy: &EventLoopProxyWrapper) -> Result {
        let mut state = self.state()?;
        if state.stopping || state.failed {
            return Err(io::Error::other("执行宿主已经停止").into());
        }
        if state.connection.is_some() {
            return Err(io::Error::other("执行连接已经建立，不能重复启动").into());
        }
        let proxy = (**proxy).clone();
        let notify = Arc::new(move || proxy.send_event(WinitUserEvent::WakeUp).is_ok());
        let wake_window = notify.clone();
        let wake_ecs = notify.clone();
        let (ecs_access, ecs_requests) = EcsAccess::channel(move || wake_ecs());
        state.connection = Some(Connection::start(environment, ecs_access, move || {
            wake_window()
        })?);
        state.ecs_requests = Some(ecs_requests);
        state.notify = Some(notify);
        debug!("共享执行连接已建立");
        Ok(())
    }

    pub(super) fn next_ecs_request(&self) -> Result<Option<EcsRequest>> {
        let mut state = self.state()?;
        let Some(requests) = &state.ecs_requests else {
            return Ok(None);
        };
        match requests.try_recv() {
            Ok(request) => Ok(Some(request)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => {
                state.ecs_requests = None;
                Ok(None)
            }
        }
    }

    fn submit<T>(
        &self,
        action: impl FnOnce(&mut Connection) -> std::result::Result<T, BridgeFailure>,
    ) -> Result<T> {
        let mut state = self.state()?;
        if state.stopping || state.failed {
            return Err(io::Error::other("执行宿主已经停止").into());
        }
        let connection = state
            .connection
            .as_mut()
            .ok_or_else(|| io::Error::other("执行连接尚未建立"))?;
        action(connection).map_err(BevyError::from)
    }

    pub fn begin_login(&self) -> Result<u64> {
        self.submit(Connection::begin_login)
    }
    pub fn cancel_login(&self, id: u64) -> Result {
        self.submit(|connection| connection.cancel_login(id))
    }
    pub fn logout(&self) -> Result<u64> {
        self.submit(Connection::logout)
    }
    pub fn request_memory(
        &self,
        request: agentelier_bridge_connection::MemoryRequest,
    ) -> Result<u64> {
        self.submit(|connection| connection.request_memory(request))
    }

    pub fn request_image_attachment(
        &self,
        session: SessionId,
        call: String,
        action: agentelier_bridge_connection::ImageAttachmentAction,
    ) -> Result<u64> {
        self.submit(|connection| connection.request_image_attachment(session, call, action))
    }

    pub fn change_audio_draft(&self, session: SessionId, path: Option<PathBuf>) -> Result<u64> {
        self.submit(|connection| connection.change_audio_draft(session, path))
    }

    pub fn transcribe_audio(&self, session: SessionId, audio: SessionId) -> Result<u64> {
        self.submit(|connection| connection.transcribe_audio(session, audio))
    }

    pub fn cancel_transcription(&self, session: SessionId, id: u64) -> Result<bool> {
        self.submit(|connection| connection.cancel_transcription(session, id))
    }

    pub fn mcp_config(&self, change: agentelier_bridge_connection::ConfigChange) -> Result<u64> {
        self.submit(|connection| connection.mcp_config(change))
    }

    pub fn manage_skills(
        &self,
        request: agentelier_bridge_connection::SkillRequest,
    ) -> Result<u64> {
        self.submit(|connection| connection.manage_skills(request))
    }

    pub fn mcp_material(
        &self,
        request: agentelier_bridge_connection::MaterialRequest,
    ) -> Result<u64> {
        self.submit(|connection| connection.mcp_material(request))
    }

    pub fn cancel_mcp_material(&self, id: u64) -> Result {
        self.submit(|connection| connection.cancel_mcp_material(id))
    }

    pub fn request_model(&self, request: agentelier_llm_model_bridge::ModelRequest) -> Result<u64> {
        let id = self.submit(|connection| connection.request_model(request))?;
        self.state()?.model_busy = true;
        Ok(id)
    }
    pub fn request_conversation(
        &self,
        request: agentelier_conversation_bridge::ConversationRequest,
    ) -> Result<u64> {
        let id = self.submit(|connection| connection.request_conversation(request))?;
        self.state()?.model_busy = true;
        Ok(id)
    }
    pub fn change_session_metadata(
        &self,
        session: SessionId,
        change: agentelier_conversation_bridge::SessionMetadataChange,
    ) -> Result<u64> {
        self.submit(|connection| connection.change_session_metadata(session, change))
    }
    pub fn change_session_model(&self, session: SessionId, slug: String) -> Result<u64> {
        self.submit(|connection| connection.change_session_model(session, slug))
    }
    pub fn refresh_archives(&self) -> Result {
        self.submit(|connection| connection.refresh_archives())
    }
    pub fn change_project(
        &self,
        change: agentelier_conversation_bridge::ProjectChange,
    ) -> Result<u64> {
        self.submit(|connection| connection.change_project(change))
    }
    pub fn cancel_conversation(
        &self,
        session: SessionId,
        turn: u64,
        operation: u64,
    ) -> Result<bool> {
        self.submit(|connection| connection.cancel_conversation(session, turn, operation))
    }
    pub fn steer_conversation(
        &self,
        session: SessionId,
        turn: u64,
        operation: u64,
        text: String,
    ) -> Result<u64> {
        self.submit(|connection| connection.steer_conversation(session, turn, operation, text))
    }

    pub fn request_session(
        &self,
        action: agentelier_conversation_bridge::SessionAction,
    ) -> Result<u64> {
        self.submit(|connection| connection.request_session(action))
    }

    pub fn request_task_action(
        &self,
        session: SessionId,
        action: agentelier_conversation_bridge::TaskAction,
    ) -> Result<u64> {
        self.submit(|connection| connection.request_task_action(session, action))
    }

    pub fn model_busy(&self) -> Result<bool> {
        Ok(self.state()?.model_busy)
    }

    pub fn is_stopping(&self) -> Result<bool> {
        Ok(self.state()?.stopping)
    }

    pub fn begin_shutdown(&self) -> Result {
        let mut state = self.state()?;
        let first_request = !state.stopping;
        state.stopping = true;
        if let Some(connection) = &mut state.connection {
            connection.begin_shutdown()?;
            if first_request {
                debug!("已停止共享业务接收与普通唤醒，等待执行体结束");
            }
        }
        Ok(())
    }

    pub fn is_finished(&self) -> Result<bool> {
        Ok(self
            .state()?
            .connection
            .as_ref()
            .is_none_or(Connection::is_finished))
    }

    pub fn finish(&self) -> Result {
        self.begin_shutdown()?;
        let mut state = self.state()?;
        if let Some(connection) = &mut state.connection {
            connection.finish()?;
        }
        state.ecs_requests = None;
        Ok(())
    }
}

fn receive(world: &mut World) -> Result {
    let host = world.resource::<AgentHost>().clone();
    let (events, failure) = {
        let mut state = host.state()?;
        if state.stopping || state.failed {
            return Ok(());
        }
        let mut events = Vec::new();
        let mut failure = None;
        if let Some(connection) = &mut state.connection {
            for _ in 0..MAX_EVENTS_PER_UPDATE {
                match connection.try_receive() {
                    Ok(Some(event)) => events.push(event),
                    Ok(None) => break,
                    Err(error) => {
                        failure = Some(error.to_string());
                        break;
                    }
                }
            }
        }
        if events.len() == MAX_EVENTS_PER_UPDATE
            && state.notify.as_ref().is_some_and(|notify| !notify())
        {
            failure = Some("执行结果已入队，但无法继续唤醒界面".into());
        }
        state.failed = failure.is_some();
        (events, failure)
    };
    for event in events {
        match event.kind {
            BridgeEventKind::Account(kind) => {
                world.write_message(AccountEvent {
                    operation_id: event.operation_id,
                    kind,
                });
            }
            BridgeEventKind::Conversation(kind) => {
                let busy = host.submit(|connection| Ok(connection.model_busy()))?;
                host.state()?.model_busy = busy;
                match *kind {
                    agentelier_conversation_bridge::ConversationEvent::GlobalTasks(snapshot) => {
                        world.write_message(GlobalTasksEvent(snapshot));
                    }
                    agentelier_conversation_bridge::ConversationEvent::TaskNotification(
                        notification,
                    ) => {
                        world.write_message(GlobalTaskNotificationEvent(notification));
                    }
                    kind => {
                        world.write_message(ConversationEvent {
                            operation_id: event.operation_id,
                            kind,
                        });
                    }
                }
            }
            BridgeEventKind::McpMaterial(result) => {
                world.write_message(McpMaterialEvent {
                    operation_id: event.operation_id,
                    result,
                });
            }
            BridgeEventKind::Memory(result) => {
                world.write_message(MemoryEvent {
                    operation_id: event.operation_id,
                    progress: false,
                    result,
                });
            }
            BridgeEventKind::MemoryProgress(view) => {
                world.write_message(MemoryEvent {
                    operation_id: event.operation_id,
                    progress: true,
                    result: Ok(agentelier_bridge_connection::MemoryReply::Chat(view)),
                });
            }
            BridgeEventKind::MemoryWork(status) => {
                world.write_message(MemoryWorkEvent(status));
            }
            BridgeEventKind::ImageAttachment(result) => {
                world.write_message(ImageAttachmentEvent {
                    operation_id: event.operation_id,
                    result,
                });
            }
            BridgeEventKind::McpStatus(status) => {
                world.write_message(McpStatusEvent(status));
            }
            BridgeEventKind::Skills(result) => {
                world.write_message(SkillsEvent {
                    operation_id: event.operation_id,
                    result,
                });
            }
            BridgeEventKind::McpConfig(result) => {
                world.write_message(McpConfigEvent {
                    operation_id: event.operation_id,
                    result,
                });
            }
            BridgeEventKind::Model(kind) => {
                let busy = host.submit(|connection| Ok(connection.model_busy()))?;
                host.state()?.model_busy = busy;
                world.write_message(ModelEvent {
                    operation_id: event.operation_id,
                    kind,
                });
            }
        }
    }
    if let Some(reason) = failure {
        host.state()?.model_busy = false;
        world.write_message(HostFailed { reason });
    }
    let busy = host.model_busy()?;
    if world.resource::<HostActivity>().model_busy != busy {
        world.resource_mut::<HostActivity>().model_busy = busy;
    }
    Ok(())
}
