use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostFailed, ModelEvent, ReceiveAgentEvents};
use agentelier_llm_model_bridge::{ModelEventKind, ModelRequest, ModelResult};
use agentelier_login_ui::AccountState;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use std::io;
use tracing::{error, info};

const PANEL_GAP: f32 = 12.0;

#[derive(Clone, Reflect)]
pub struct ModelChoice {
    pub slug: String,
    pub display_name: String,
    pub supported_reasoning_levels: Vec<String>,
    pub default_reasoning_level: Option<String>,
    pub image_input: Option<bool>,
    pub context_window: Option<u64>,
    pub effective_window: Option<u64>,
    pub auto_compact_limit: Option<u64>,
    pub comp_hash: Option<String>,
}

#[derive(Resource, Reflect, Clone, Default)]
#[reflect(Resource)]
pub struct ModelState {
    pub models: Vec<ModelChoice>,
    pub operation: Option<u64>,
    pub catalog_loaded: bool,
    pub catalog_error: Option<String>,
    pub account_identity: Option<(String, String)>,
}

impl ModelState {
    pub(crate) fn can_request(&self, account: &AccountState) -> bool {
        self.operation.is_none()
            && !account.runtime_failed
            && account.model_unavailable_reason().is_none()
    }
    pub(crate) fn can_create_session(&self, account: &AccountState) -> bool {
        self.can_request(account) && self.catalog_loaded && self.catalog_error.is_none()
    }
}

#[derive(Component, Clone, Default)]
pub struct ChatUiHost;

pub fn install(
    app: &mut App,
    model_defaults: impl Fn() -> io::Result<crate::SessionModelDefaults> + Send + Sync + 'static,
) -> Result {
    crate::archives::install(app)?;
    crate::projects::install(app);
    app.add_plugins(agentelier_markdown::MarkdownPlugin);
    crate::reading::install(app);
    app.insert_resource(crate::conversation::SessionDefaultsSource(Box::new(
        model_defaults,
    )));
    crate::conversation::install(app);
    crate::spinner::install(app);
    crate::global_tasks::install(app);
    crate::background_tasks::install(app);
    crate::task_toast::install(app);
    crate::prompts::install(app)?;
    crate::mcp::install(app)?;
    crate::memory::install(app);
    crate::skills::install(app)?;
    app.init_resource::<ModelState>()
        .register_type::<ModelState>()
        .add_observer(mount)
        .add_systems(Update, receive.after(ReceiveAgentEvents))
        .add_systems(Update, auto_catalog.after(receive));
    Ok(())
}

fn mount(event: On<Add<ChatUiHost>>, mut commands: Commands) {
    let host = event.entity;
    commands.queue(move |world: &mut World| {
        let chat = world.resource::<ChatState>();
        let content = bsn_list! {
            Name("ModelPreparation")
            Node { width: percent(100), height: percent(100), min_height: px(0), flex_direction: FlexDirection::Column, row_gap: px(PANEL_GAP) }
            Children [
                crate::prompts::NormalContent
                Node { width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column }
                Children [ @crate::conversation::panel(chat) ]--
                crate::prompts::PromptWorkspace Name("PromptWorkspace")
                Node { display: Display::None, width: percent(100), flex_grow: 1.0, min_height: px(0), flex_direction: FlexDirection::Column }
            ]
        };
        let archives = bsn_list! { @crate::archives::panel(world) };
        world.commands().spawn_scene_with_error_handler(bsn! {
            @crate::workspace::scene(content, archives) ChildOf(host)
        });
    });
}

fn auto_catalog(world: &mut World) -> Result {
    let state = world.resource::<ModelState>();
    let account = world.resource::<AccountState>();
    if world.resource::<crate::ArchiveState>().loading
        || state.catalog_loaded
        || state.catalog_error.is_some()
        || !state.can_request(account)
        || world.resource::<AgentHost>().model_busy()?
    {
        return Ok(());
    }
    let result = world
        .resource::<AgentHost>()
        .request_model(ModelRequest::Catalog);
    let mut state = world.resource_mut::<ModelState>();
    match result {
        Ok(id) => {
            state.operation = Some(id);
            info!(operation_id = id, "开始读取模型目录");
        }
        Err(failure) => {
            state.catalog_error = Some(failure.to_string());
            error!(error = %failure, "无法开始读取模型目录");
        }
    }
    Ok(())
}

pub(crate) fn receive(world: &mut World, mut failures: Local<MessageCursor<HostFailed>>) -> Result {
    let account = world.resource::<AccountState>();
    let identity = account
        .model_identity()
        .map(|(subject, workspace)| (subject.to_owned(), workspace.to_owned()));
    if world.resource::<ModelState>().account_identity != identity
        || identity.is_none() && world.resource::<ModelState>().catalog_loaded
    {
        *world.resource_mut::<ModelState>() = ModelState {
            account_identity: identity,
            ..default()
        };
    }
    let events: Vec<_> = world
        .resource_mut::<Messages<ModelEvent>>()
        .drain()
        .collect();
    for event in events {
        if world.resource::<ModelState>().operation != Some(event.operation_id) {
            continue;
        }
        let mut state = world.resource_mut::<ModelState>();
        state.operation = None;
        match event.kind {
            ModelEventKind::Completed(Ok(ModelResult::Catalog(models))) => {
                state.models = models
                    .into_iter()
                    .map(|model| ModelChoice {
                        slug: model.slug,
                        display_name: model.display_name,
                        supported_reasoning_levels: model.supported_reasoning_levels,
                        default_reasoning_level: model.default_reasoning_level,
                        image_input: model.image_input,
                        context_window: model.context_limits.window,
                        effective_window: model.context_limits.effective,
                        auto_compact_limit: model.context_limits.automatic,
                        comp_hash: model.context_limits.comp_hash,
                    })
                    .collect();
                state.catalog_loaded = true;
                state.catalog_error = None;
                info!(model_count = state.models.len(), "模型目录读取完成");
            }
            ModelEventKind::Completed(Ok(ModelResult::Verified(_))) => {
                let reason = "模型目录请求收到不匹配的结果";
                error!(operation_id = event.operation_id, "{reason}");
                state.catalog_error = Some(reason.into());
            }
            ModelEventKind::Completed(Err(failure)) => {
                error!(
                    operation_id = event.operation_id,
                    status = failure.status,
                    code = failure.code,
                    "模型目录读取失败"
                );
                state.catalog_error = Some(failure.to_string());
            }
            ModelEventKind::Cancelled => {
                state.catalog_error = Some("模型目录读取已停止".into());
            }
        }
    }
    if let Some(failure) = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|failure| failure.reason.clone())
    {
        let mut state = world.resource_mut::<ModelState>();
        state.operation = None;
        state.catalog_error = Some(failure);
    }
    Ok(())
}
