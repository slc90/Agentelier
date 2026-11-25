use crate::ModelState;
use crate::conversation::{self, ChatSession, ChatTurnStatus, SessionDefaultsSource};
use agentelier_agent_host::{
    AgentHost, HostFailed, MemoryEvent, MemoryReply, MemoryRequest, ReceiveAgentEvents,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_login_ui::AccountState;
use bevy::{
    prelude::*,
    text::LineBreak,
    ui::{Display, UiSystems},
    ui_widgets::Activate,
};
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryTextField, WidgetryTextFieldHeadless};

#[derive(Resource, Default)]
struct State {
    session: Option<ChatSession>,
    pending: Option<u64>,
    submitted: Option<String>,
    clear_editor: bool,
    draft: String,
    status: String,
    rows_changed: bool,
    awaiting_catalog: bool,
}

#[derive(Component, Default, Clone)]
pub(super) struct Composer;
#[derive(Component, Default, Clone)]
struct Messages;
#[derive(Component, Default, Clone)]
struct Status;
#[derive(Component, Default, Clone)]
struct Retry;

pub(super) fn install(app: &mut App) {
    app.init_resource::<State>()
        .add_systems(Update, receive.after(ReceiveAgentEvents))
        .add_systems(Update, ready_catalog.after(crate::preparation::receive))
        .add_systems(
            PostUpdate,
            (remember_draft, refresh).chain().before(UiSystems::Content),
        );
}

pub(super) fn open(world: &mut World) {
    if world.resource::<State>().pending.is_some() {
        return;
    }
    if world.resource::<State>().session.is_none() && !world.resource::<ModelState>().catalog_loaded
    {
        let models = world.resource::<ModelState>();
        let error = models.catalog_error.clone().or_else(|| {
            world
                .resource::<AccountState>()
                .model_unavailable_reason()
                .map(str::to_owned)
        });
        let mut state = world.resource_mut::<State>();
        state.awaiting_catalog = error.is_none();
        state.status = error.unwrap_or_else(|| "正在等待当前账号的模型目录…".into());
        return;
    }
    let result = (|| -> Result<u64> {
        let defaults = (world.resource::<SessionDefaultsSource>().0)()?;
        world
            .resource::<AgentHost>()
            .request_memory(MemoryRequest::OpenChat {
                slug: defaults.slug,
                effort: defaults.effort,
            })
    })();
    let mut state = world.resource_mut::<State>();
    state.awaiting_catalog = false;
    match result {
        Ok(id) => {
            state.pending = Some(id);
            state.status = "正在打开记忆会话…".into();
        }
        Err(failure) => state.status = failure.to_string(),
    }
}

fn ready_catalog(
    model: Res<ModelState>,
    state: Res<State>,
    composers: Query<(), With<Composer>>,
    mut commands: Commands,
) {
    if state.awaiting_catalog
        && !composers.is_empty()
        && (model.catalog_loaded || model.catalog_error.is_some())
    {
        commands.queue(open);
    }
}

fn receive(
    mut events: MessageReader<MemoryEvent>,
    mut failures: MessageReader<HostFailed>,
    mut state: ResMut<State>,
) -> Result {
    for event in events.read() {
        if state.pending != Some(event.operation_id) {
            continue;
        }
        if !event.progress {
            state.pending = None;
        }
        match &event.result {
            Ok(MemoryReply::Chat(view)) => {
                let session = conversation::project(view.as_ref().clone())?;
                state.status = match session.turn.as_ref().map(|turn| &turn.status) {
                    Some(ChatTurnStatus::Running) => "正在处理记忆消息…".into(),
                    Some(ChatTurnStatus::Failed(reason)) => reason.clone(),
                    Some(ChatTurnStatus::Cancelled) => "记忆消息已停止".into(),
                    Some(ChatTurnStatus::Limited) => "记忆会话达到本轮执行限制".into(),
                    _ => String::new(),
                };
                state.session = Some(session);
                state.rows_changed = true;
                if let Some(input) = state.submitted.take()
                    && state.draft == input
                {
                    state.draft.clear();
                    state.clear_editor = true;
                }
            }
            Err(failure) => {
                state.submitted = None;
                state.status.clone_from(failure);
            }
            _ => {}
        }
    }
    for failure in failures.read() {
        state.pending = None;
        state.submitted = None;
        state.status.clone_from(&failure.reason);
    }
    Ok(())
}

fn remember_draft(world: &mut World) -> Result {
    let Ok(field) = world
        .query_filtered::<Entity, With<Composer>>()
        .single(world)
    else {
        return Ok(());
    };
    if world.resource::<State>().clear_editor {
        WidgetryTextFieldHeadless::set_text_in_world(world, field, "")
            .map_err(|failure| failure.into_bevy_error())?;
        world.resource_mut::<State>().clear_editor = false;
        return Ok(());
    }
    let draft = WidgetryTextFieldHeadless::text(world, field)
        .map_err(|failure| failure.into_bevy_error())?;
    if draft != world.resource::<State>().draft {
        world.resource_mut::<State>().draft = draft;
    }
    Ok(())
}

fn refresh(world: &mut World) -> Result {
    let Some(list) = world
        .query_filtered::<Entity, With<Messages>>()
        .iter(world)
        .next()
    else {
        return Ok(());
    };
    let state = world.resource::<State>();
    let render = state.rows_changed || world.get::<Children>(list).is_none();
    let busy = state.pending.is_some() || state.awaiting_catalog;
    let session = state.session.is_some();
    let status = state.status.clone();
    if render {
        let session = state.session.clone();
        conversation::reconcile_list(world, list, session.as_ref(), None)?;
        world.resource_mut::<State>().rows_changed = false;
    }
    if let Ok(mut text) = world
        .query_filtered::<&mut Text, With<Status>>()
        .single_mut(world)
    {
        text.set_if_neq(Text(status));
    }
    if let Ok(mut button) = world
        .query_filtered::<&mut Node, With<Retry>>()
        .single_mut(world)
    {
        let display = if !session && !busy {
            Display::Flex
        } else {
            Display::None
        };
        if button.display != display {
            button.display = display;
        }
    }
    Ok(())
}

pub(super) fn submit(world: &mut World) -> Result {
    remember_draft(world)?;
    let state = world.resource::<State>();
    if state.pending.is_some() || state.awaiting_catalog || state.draft.trim().is_empty() {
        return Ok(());
    }
    let Some(session) = state.session.as_ref().map(|session| session.id) else {
        return Ok(());
    };
    let input = state.draft.clone();
    let result = world
        .resource::<AgentHost>()
        .request_memory(MemoryRequest::SendChat {
            session,
            input: input.clone(),
        });
    match result {
        Ok(id) => {
            let mut state = world.resource_mut::<State>();
            state.pending = Some(id);
            state.submitted = Some(input);
            state.status = "正在提交记忆消息…".into();
        }
        Err(failure) => world.resource_mut::<State>().status = failure.to_string(),
    }
    Ok(())
}

fn retry(_: On<Activate>, mut commands: Commands) {
    commands.queue(open);
}

pub(super) fn panel(world: &World) -> impl Scene + use<> {
    let draft = world.resource::<State>().draft.clone();
    bsn! {
        Name("MemoryChat")
        Node { width: percent(100), min_height: px(0), flex_grow: 1.0, flex_direction: FlexDirection::Column, row_gap: px(8) }
        Children [
            @WidgetryScrollArea {
                @children: bsn_list! {
                    Messages Name("MemoryChatMessages")
                    Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(8) }
                }
            }
            Name("MemoryChatScroll") crate::reading::MemoryReadingScroll
            Node { width: percent(100), min_height: px(0), flex_grow: 1.0 }--
            Status Name("MemoryChatStatus") Text("") WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
            @WidgetryTextField {
                @value: draft, @multiline: true,
                @auto_size: {Some(AutoSize { min_rows: 2, max_rows: 6 })},
            }
            Composer Name("MemoryChatComposer") Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }--
            @button("重试打开会话") Retry Name("RetryMemoryChat") on(retry)
            Node { align_self: AlignSelf::Start }
        ]
    }
}

fn button(label: &'static str) -> impl Scene {
    bsn! {
        @AgentelierButton { @style: AgentelierButtonStyle::Primary, @children: bsn_list! { Text(label) WidgetryText } }
        Node { flex_shrink: 0.0 }
    }
}
