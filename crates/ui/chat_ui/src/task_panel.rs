use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_conversation_bridge::{
    SessionId, SessionProcess, SessionProcesses, TaskAction, TaskActionOutcome, TaskActionReceipt,
    TaskId,
};
use bevy::{prelude::*, text::LineBreak, ui_widgets::Activate};
use bevy_widgetry::{
    button::WidgetryButton, scene::WidgetrySceneCommandsExt, scroll_area::WidgetryScrollArea,
    text::WidgetryText,
};
use std::collections::BTreeMap;

const TASK_VIEWPORT_HEIGHT: f32 = 116.0;
const ROW_GAP: f32 = 5.0;
const MAX_PREVIEW_CHARACTERS: usize = 1024;

#[derive(Resource, Default)]
struct PanelState {
    session: Option<SessionId>,
    revision: Option<u64>,
    processes: Vec<SessionProcess>,
    paused: bool,
    external: Vec<String>,
    history: bool,
    feedback: String,
    pending: BTreeMap<u64, TaskAction>,
}

#[derive(Component, Clone, Default)]
struct Panel;
#[derive(Component, Clone, Default)]
struct Summary;
#[derive(Component, Clone, Default)]
struct List;
#[derive(Component, Clone, Default)]
struct Viewport;
#[derive(Component, Clone, Default)]
struct Feedback;
#[derive(Component, Clone, Default)]
struct RowLabel;

#[derive(Clone, PartialEq, Eq)]
enum Intent {
    History,
    Results,
    Control(TaskAction),
    ConfirmExternal,
}

#[derive(Component, Clone)]
struct Button {
    session: Option<SessionId>,
    intent: Intent,
}

#[derive(Component, Clone, PartialEq, Eq)]
struct Row {
    session: SessionId,
    id: String,
    actions: Vec<(&'static str, TaskAction)>,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PanelState>().add_systems(
        PostUpdate,
        reconcile
            .run_if(|state: Res<PanelState>, chat: Res<ChatState>| {
                state.is_changed() || chat.is_changed()
            })
            .before(bevy::ui::UiSystems::Content),
    );
}

pub(crate) fn bind(world: &mut World, session: Option<SessionId>) {
    *world.resource_mut::<PanelState>() = PanelState {
        session,
        ..default()
    };
}

pub(crate) fn policy(world: &mut World, paused: bool, external: Vec<String>) {
    let mut state = world.resource_mut::<PanelState>();
    state.paused = paused;
    state.external = external;
}

pub(crate) fn processes(world: &mut World, update: SessionProcesses) {
    let mut state = world.resource_mut::<PanelState>();
    if state.session == Some(update.session)
        && state
            .revision
            .is_none_or(|current| update.revision > current)
    {
        state.revision = Some(update.revision);
        state.processes = update.processes;
    }
}

pub(crate) fn receipt(world: &mut World, id: u64, receipt: TaskActionReceipt) {
    let mut state = world.resource_mut::<PanelState>();
    if state.session != Some(receipt.session) || state.pending.remove(&id).is_none() {
        return;
    }
    state.feedback = match receipt.result {
        Ok(TaskActionOutcome::Status(status)) => status,
        Ok(TaskActionOutcome::ProcessOutput(output)) => {
            let mut text = output
                .chars()
                .take(MAX_PREVIEW_CHARACTERS)
                .collect::<String>();
            if text.len() < output.len() {
                text.push('…');
            }
            text
        }
        Err(reason) => reason,
    };
}

fn button(label: &'static str, name: String, button: Button) -> impl Scene {
    bsn! {
        @WidgetryButton {@children:{Some(Box::new(bsn_list! {Text(label) WidgetryText}) as Box<dyn SceneList>)}}
        Name(name) template(move |_| Ok(button.clone())) on(activate)
    }
}

fn header(label: &'static str, name: &'static str, intent: Intent) -> impl Scene {
    button(
        label,
        name.to_owned(),
        Button {
            session: None,
            intent,
        },
    )
}

pub(crate) fn panel() -> impl Scene {
    bsn! {
        Panel Name("ChatTaskPanel")
        Node {display:Display::None,width:percent(100),min_width:px(0),flex_direction:FlexDirection::Column,row_gap:px(ROW_GAP)}
        Children [
            Node {column_gap:px(ROW_GAP),row_gap:px(ROW_GAP),align_items:AlignItems::Center,flex_wrap:FlexWrap::Wrap}
            Children [
                Text("") WidgetryText Summary Name("ChatTaskSummary")--
                @header("任务历史", "ChatTaskHistory", Intent::History)--
                @header("停止全部后台工作", "ChatTaskStopAll", Intent::Control(TaskAction::StopAll))--
                @header("恢复后台工作", "ChatTaskResume", Intent::Control(TaskAction::Resume))--
                @header("确认继续执行", "ChatTaskConfirmExternal", Intent::ConfirmExternal)--
                @header("处理已完成结果", "ChatTaskProcessResults", Intent::Results)
            ]--
            @WidgetryScrollArea {@children:{Some(Box::new(bsn_list! {
                List Name("ChatTaskList")
                Node {width:percent(100),min_width:px(0),flex_direction:FlexDirection::Column,row_gap:px(ROW_GAP)}
            }) as Box<dyn SceneList>)}}
            Viewport Node {width:percent(100),height:px(TASK_VIEWPORT_HEIGHT),min_height:px(0),flex_shrink:1.0}--
            Text("") WidgetryText Feedback Name("ChatTaskFeedback") TextLayout::linebreak(LineBreak::AnyCharacter)
        ]
    }
}

fn activate(event: On<Activate>, actions: Query<&Button>, mut commands: Commands) -> Result {
    let button = actions.get(event.entity)?.clone();
    commands.queue(move |world: &mut World| {
        let state = world.resource::<PanelState>();
        let Some(session) = state
            .session
            .filter(|id| button.session.is_none_or(|owner| owner == *id))
        else {
            return;
        };
        let action = match button.intent {
            Intent::History => {
                let mut state = world.resource_mut::<PanelState>();
                state.history = !state.history;
                return;
            }
            Intent::Results => {
                super::conversation::process_results(world, session);
                return;
            }
            Intent::ConfirmExternal => TaskAction::ConfirmExternal {
                call_ids: state.external.clone(),
            },
            Intent::Control(action) => action,
        };
        if state.pending.values().any(|pending| pending == &action) {
            return;
        }
        let result = world
            .resource::<AgentHost>()
            .request_task_action(session, action.clone());
        let mut state = world.resource_mut::<PanelState>();
        match result {
            Ok(id) => {
                state.pending.insert(id, action);
                state.feedback = "正在确认任务控制…".into();
            }
            Err(failure) => state.feedback = failure.to_string(),
        }
    });
    Ok(())
}

pub(crate) fn reconcile(world: &mut World) -> Result {
    let chat = world.resource::<ChatState>();
    let state = world.resource::<PanelState>();
    let running = chat.operation.is_some();
    let ready = chat
        .tasks
        .iter()
        .filter(|task| task.delivery == "pending")
        .count();
    let active = chat
        .tasks
        .iter()
        .filter(|task| matches!(task.status.as_str(), "pending" | "running"))
        .count();
    let live_processes = state
        .processes
        .iter()
        .filter(|p| p.phase == "running")
        .count();
    let visible = state.session.is_some()
        && (running
            || !chat.tasks.is_empty()
            || !state.processes.is_empty()
            || state.paused
            || !state.external.is_empty()
            || !state.feedback.is_empty());
    let summary = format!("后台任务 {active} · 进程 {live_processes} · 已保存，待处理 {ready}");
    let feedback = state.feedback.clone();
    let mut desired = Vec::new();
    if let Some(session) = state.session {
        for task in &chat.tasks {
            if task.delivery == "delivered"
                && !state.history
                && !state.processes.iter().any(|process| {
                    process.call_id == task.call_id
                        && (process.readable || process.phase == "running")
                })
            {
                continue;
            }
            let mut actions = Vec::new();
            if !task.cancelling
                && matches!(task.status.as_str(), "pending" | "running")
                && let Some(instance) = task
                    .instance
                    .as_ref()
                    .and_then(|id| id.parse::<SessionId>().ok())
                && let Ok(task_id) = task.id.parse::<SessionId>()
            {
                actions.push((
                    "取消任务",
                    TaskAction::Cancel {
                        task: TaskId(task_id),
                        instance,
                    },
                ));
            }
            let label = if task.cancelling {
                "取消中"
            } else {
                status(&task.status)
            };
            let delivery = match task.delivery.as_str() {
                "saved" => "已保存，待归并",
                "pending" => "已保存，待处理",
                "delivered" => "已用于成功请求",
                _ => "等待保存",
            };
            let bytes = task
                .argument_bytes
                .map_or_else(String::new, |count| format!(" · 参数 {count} B"));
            let waiting = if task.status == "pending" {
                state
                    .processes
                    .iter()
                    .find(|p| p.phase == "running")
                    .map_or_else(
                        || " · 等待准入".to_owned(),
                        |p| format!(" · 等待会话许可，进程 {}", p.id),
                    )
            } else {
                String::new()
            };
            desired.push((
                Row {
                    session,
                    id: task.id.clone(),
                    actions,
                },
                format!(
                    "{} · Turn {} · {} · {} · {}{bytes}{waiting}\n{}",
                    task.tool, task.turn, task.source, label, delivery, task.id
                ),
            ));
        }
        for process in &state.processes {
            if process.phase == "exited" && !process.readable && !state.history {
                continue;
            }
            let action = |kind: fn(String, SessionId) -> TaskAction| {
                kind(process.id.clone(), process.instance)
            };
            let actions = if process.phase == "running" {
                vec![(
                    "终止进程",
                    action(|process, instance| TaskAction::Terminate { process, instance }),
                )]
            } else if process.readable {
                vec![
                    (
                        "读取尾部",
                        action(|process, instance| TaskAction::ReadExit { process, instance }),
                    ),
                    (
                        "收起尾部",
                        action(|process, instance| TaskAction::DiscardExit { process, instance }),
                    ),
                ]
            } else {
                Vec::new()
            };
            let state = match process.phase {
                "running" => "进程运行中",
                "exited" if process.readable => "进程已退出，尾部可读取",
                "exited" => "历史退出摘要",
                "cleanup_unconfirmed" => "回收未确认",
                _ => "历史退出未确认",
            };
            let exit = process
                .exit_code
                .map_or_else(String::new, |code| format!(" · 退出码 {code}"));
            let truncated = if process.truncated {
                " · 输出已截断"
            } else {
                ""
            };
            let replaced = if process.decoding_replaced {
                " · 已替换无效字符"
            } else {
                ""
            };
            desired.push((
                Row {
                    session,
                    id: process.id.clone(),
                    actions,
                },
                format!(
                    "{state} · Turn {}{exit}{truncated}{replaced}\n{}",
                    process.turn, process.id
                ),
            ));
        }
    }
    let pending = state.pending.values().cloned().collect::<Vec<_>>();
    let paused = state.paused;
    let external = !state.external.is_empty();
    for mut node in world
        .query_filtered::<&mut Node, With<Panel>>()
        .iter_mut(world)
    {
        node.display = if visible {
            Display::Flex
        } else {
            Display::None
        };
    }
    for mut node in world
        .query_filtered::<&mut Node, With<Viewport>>()
        .iter_mut(world)
    {
        node.display = if desired.is_empty() {
            Display::None
        } else {
            Display::Grid
        };
    }
    for mut text in world
        .query_filtered::<&mut Text, With<Summary>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(summary.clone()));
    }
    for mut text in world
        .query_filtered::<&mut Text, With<Feedback>>()
        .iter_mut(world)
    {
        text.set_if_neq(Text(feedback.clone()));
    }
    let buttons = world
        .query::<(Entity, &Button)>()
        .iter(world)
        .map(|(id, b)| (id, b.clone()))
        .collect::<Vec<_>>();
    for (id, b) in buttons {
        let show = match &b.intent {
            Intent::Results => ready != 0 && !running,
            Intent::ConfirmExternal => external,
            Intent::Control(TaskAction::StopAll) => {
                !paused && (running || active != 0 || live_processes != 0)
            }
            Intent::Control(TaskAction::Resume) => paused,
            _ => true,
        };
        if let Some(mut node) = world.get_mut::<Node>(id) {
            node.display = if show { Display::Flex } else { Display::None };
        }
        let disabled = match &b.intent {
            Intent::ConfirmExternal => pending
                .iter()
                .any(|item| matches!(item, TaskAction::ConfirmExternal { .. })),
            Intent::Control(action) => pending.contains(action),
            _ => false,
        };
        if disabled {
            world.entity_mut(id).insert(bevy::ui::InteractionDisabled);
        } else {
            world
                .entity_mut(id)
                .remove::<bevy::ui::InteractionDisabled>();
        }
    }
    reconcile_rows(world, desired)
}

fn reconcile_rows(world: &mut World, desired: Vec<(Row, String)>) -> Result {
    let lists = world
        .query_filtered::<Entity, With<List>>()
        .iter(world)
        .collect::<Vec<_>>();
    for parent in lists {
        let existing = world
            .get::<Children>(parent)
            .into_iter()
            .flatten()
            .copied()
            .filter_map(|id| world.get::<Row>(id).map(|row| (id, row.clone())))
            .collect::<Vec<_>>();
        for (entity, row) in &existing {
            if !desired.iter().any(|(wanted, _)| wanted == row) {
                world.entity_mut(*entity).despawn();
            }
        }
        let mut ordering = Vec::new();
        for (row, text) in &desired {
            if let Some((entity, _)) = existing.iter().find(|(_, old)| old == row) {
                ordering.push(*entity);
                let label = world
                    .get::<Children>(*entity)
                    .into_iter()
                    .flatten()
                    .copied()
                    .find(|child| world.get::<RowLabel>(*child).is_some());
                if let Some(label) = label
                    && let Some(mut current) = world.get_mut::<Text>(label)
                {
                    current.set_if_neq(Text(text.clone()));
                }
                continue;
            }
            let name = format!("ChatTaskRow-{}", row.id);
            let key = row.clone();
            let text = text.clone();
            let entity = world.commands().spawn_scene_with_error_handler(bsn! {
                Name(name) ChildOf(parent) template(move |_| Ok(key.clone()))
                Node {width:percent(100),min_width:px(0),column_gap:px(ROW_GAP),align_items:AlignItems::Center,flex_shrink:0.0,flex_wrap:FlexWrap::Wrap}
                Children [Text(text) WidgetryText RowLabel TextLayout::linebreak(LineBreak::AnyCharacter) Node {flex_grow:1.0,min_width:px(0)}]
            }).id();
            for (label, action) in &row.actions {
                let label = *label;
                let name = format!("ChatTaskControl-{}-{}", row.id, label);
                let descriptor = Button {
                    session: Some(row.session),
                    intent: Intent::Control(action.clone()),
                };
                world.commands().spawn_scene_with_error_handler(
                    bsn! { @button(label, name, descriptor) ChildOf(entity) },
                );
            }
            ordering.push(entity);
        }
        if world
            .get::<Children>(parent)
            .is_none_or(|children| !children.iter().eq(ordering.iter().copied()))
        {
            world.commands().entity(parent).replace_children(&ordering);
        }
    }
    Ok(())
}

fn status(status: &str) -> &str {
    match status {
        "pending" => "待执行",
        "running" => "运行中",
        "succeeded" => "成功",
        "failed" => "失败",
        "timed_out" => "超时",
        "cancelled" => "已取消",
        "not_executed" => "未执行",
        "external_unconfirmed" => "远端效果未确认",
        _ => "结果未确认",
    }
}
