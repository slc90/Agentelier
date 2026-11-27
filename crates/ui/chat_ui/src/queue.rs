use crate::active_input::PendingDraft;
use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{QueueDecision, QueueEvent, SessionAction};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;

const LIST_HEIGHT: f32 = 110.0;
const ROW_GAP: f32 = 6.0;
const PREVIEW_CHARACTERS: usize = 120;

#[derive(Clone, Reflect, PartialEq)]
pub struct ChatQueuedInput {
    pub operation_id: u64,
    pub input_id: u64,
    pub text: String,
    pub transcript: Option<crate::transcription::ChatTranscriptReference>,
}
#[derive(Component, Clone, Default)]
struct QueuePanel;
#[derive(Component, Clone, Default)]
struct QueueSummary;
#[derive(Component, Clone, Default)]
struct QueueViewport;
#[derive(Component, Clone, Default)]
struct QueueList {
    items: Vec<ChatQueuedInput>,
}
#[derive(Component, Clone, Copy)]
enum Action {
    Resume,
    Remove(u64),
}

pub(crate) fn install(app: &mut App) {
    app.add_observer(|_event: On<Add<QueueList>>, mut commands: Commands| {
        commands.queue(reconcile);
    });
}

pub(crate) fn panel() -> impl Scene {
    bsn! {
        QueuePanel Name("ChatQueuePanel")
        Node {display:Display::None, width:percent(100), min_width:px(0), min_height:px(0), flex_shrink:1.0, flex_direction:FlexDirection::Column,row_gap:px(ROW_GAP)}
        Children [
            Node {column_gap:px(ROW_GAP), align_items:AlignItems::Center}
            Children [
                Text("") WidgetryText QueueSummary--
                @AgentelierButton {@children:{Some(Box::new(bsn_list! {Text("继续队列") WidgetryText}) as Box<dyn SceneList>)}}
                Name("ResumeChatQueue") template(|_| Ok(Action::Resume)) on(activate)
            ]--
            @WidgetryScrollArea {@children:{Some(Box::new(bsn_list! {
                QueueList::default() Name("ChatQueueList")
                Node {width:percent(100),min_width:px(0),flex_direction:FlexDirection::Column,row_gap:px(ROW_GAP)}
            }) as Box<dyn SceneList>)}}
            QueueViewport Name("ChatQueueViewport")
            Node {width:percent(100),height:px(LIST_HEIGHT),min_height:px(0),flex_shrink:1.0}
        ]
    }
}
fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = *actions.get(event.entity)?;
    commands.queue(move |world: &mut World| -> Result {
        let state = world.resource::<ChatState>();
        let Some(session) = &state.session else {
            return Ok(());
        };
        let request = match action {
            Action::Resume => SessionAction::Resume {
                session: session.id,
            },
            Action::Remove(operation) => SessionAction::Remove {
                session: session.id,
                operation,
            },
        };
        match world.resource::<AgentHost>().request_session(request) {
            Ok(id) => {
                world.resource_mut::<ChatState>().queue_controls.push(id);
                tracing::info!(operation_id = id, "队列控制已提交");
            }
            Err(failure) => {
                world.resource_mut::<ChatState>().input_feedback =
                    Some(format!("队列控制未提交：{failure}"))
            }
        }
        Ok(())
    });
    Ok(())
}

pub(crate) fn receive(state: &mut ChatState, event: QueueEvent) -> Option<PendingDraft> {
    if let Some(view) = event.view
        && let Some(session) = state
            .session
            .as_mut()
            .filter(|session| session.id == view.id)
    {
        session.queue = view
            .queue
            .into_iter()
            .map(|input| ChatQueuedInput {
                operation_id: input.operation_id,
                input_id: input.input_id,
                text: input.submission.text().to_owned(),
                transcript: input
                    .submission
                    .transcript
                    .map(agentelier_conversation_bridge::TranscriptReferenceView::from)
                    .map(Into::into),
            })
            .collect();
        session.audio_draft = view.audio_draft.map(Into::into);
        session.queue_paused = view.queue_paused;
        session.execution_blocked = view.execution_blocked;
    }
    tracing::info!(
        operation_id = event.id,
        outcome = match &event.decision {
            QueueDecision::Enqueued => "enqueued",
            QueueDecision::Removed { .. } => "removed",
            QueueDecision::AlreadyStarted { .. } => "already_started",
            QueueDecision::Resumed => "resumed",
            QueueDecision::Rejected(_) => "rejected",
        },
        "队列操作已收到执行回执"
    );
    state.input_feedback = Some(match event.decision {
        QueueDecision::Enqueued => "消息已排队，启动前不进入当前任务上下文".into(),
        QueueDecision::Removed { .. } => "待办消息已移除".into(),
        QueueDecision::AlreadyStarted { .. } => "该消息已经开始，未移除；可停止当前任务".into(),
        QueueDecision::Resumed => "已继续队列".into(),
        QueueDecision::Rejected(reason) => {
            state.pending_inputs.retain(|input| input.id != event.id);
            return {
                state.input_feedback = Some(format!("队列操作未接纳：{reason}；草稿保留"));
                None
            };
        }
    });
    state
        .pending_inputs
        .iter()
        .position(|input| input.id == event.id)
        .map(|index| state.pending_inputs.remove(index))
}

pub(crate) fn reconcile(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    let items = state
        .session
        .as_ref()
        .map(|session| session.queue.clone())
        .unwrap_or_default();
    let paused = state
        .session
        .as_ref()
        .is_some_and(|session| session.queue_paused);
    let running = state.operation.is_some();
    let account = world.resource::<agentelier_login_ui::AccountState>();
    let blocked = state
        .session
        .as_ref()
        .is_none_or(|session| session.execution_blocked || !session.matches_account(account))
        || account.model_unavailable_reason().is_some();
    let mut panels = world.query_filtered::<&mut Node, With<QueuePanel>>();
    for mut node in panels.iter_mut(world) {
        node.display = if items.is_empty() && !paused {
            Display::None
        } else {
            Display::Flex
        };
    }
    let mut viewports = world.query_filtered::<&mut Node, With<QueueViewport>>();
    for mut node in viewports.iter_mut(world) {
        node.display = if items.is_empty() {
            Display::None
        } else {
            Display::Grid
        };
    }
    let mut labels = world.query_filtered::<&mut Text, With<QueueSummary>>();
    for mut text in labels.iter_mut(world) {
        text.set_if_neq(Text(format!(
            "待办消息 {} / 8 · {}",
            items.len(),
            if paused {
                "已暂停"
            } else {
                "按顺序执行"
            }
        )));
    }
    let mut buttons = world.query::<(Entity, &Action)>();
    let resumes: Vec<_> = buttons
        .iter(world)
        .filter(|(_, action)| matches!(action, Action::Resume))
        .map(|(entity, _)| entity)
        .collect();
    for entity in resumes {
        if paused && !running && !blocked {
            world
                .entity_mut(entity)
                .remove::<bevy::ui::InteractionDisabled>();
        } else {
            world
                .entity_mut(entity)
                .insert(bevy::ui::InteractionDisabled);
        }
    }
    let mut lists = world.query::<(Entity, &QueueList)>();
    let lists: Vec<_> = lists
        .iter(world)
        .filter(|(_, list)| list.items != items)
        .map(|(entity, _)| entity)
        .collect();
    for list in lists {
        if let Some(children) = world.get::<Children>(list) {
            let children = children.to_vec();
            for child in children {
                world.entity_mut(child).despawn();
            }
        }
        world
            .get_mut::<QueueList>(list)
            .ok_or_else(|| std::io::Error::other("队列列表状态丢失"))?
            .items = items.clone();
        for (index, input) in items.iter().enumerate() {
            let mut text = format!(
                "{}. {}",
                index + 1,
                input
                    .text
                    .chars()
                    .take(PREVIEW_CHARACTERS)
                    .collect::<String>()
            );
            if let Some(source) = &input.transcript {
                text.push_str(&format!("\n引用文稿 · {}", source.label()));
            }
            let operation = input.operation_id;
            let name = format!("ChatQueueRemove-{operation}");
            world.commands().spawn_scene_with_error_handler(bsn! {
                ChildOf(list) Node {width:percent(100),min_width:px(0),column_gap:px(ROW_GAP),align_items:AlignItems::Center,flex_shrink:0.0}
                Children [
                    Text(text) WidgetryText Node {flex_grow:1.0,min_width:px(0)}--
                    @AgentelierButton {@children:{Some(Box::new(bsn_list! {Text("移除") WidgetryText}) as Box<dyn SceneList>)}}
                    Name(name) template(move |_| Ok(Action::Remove(operation))) on(activate)
                ]
            });
        }
    }
    Ok(())
}
