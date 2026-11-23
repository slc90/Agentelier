use agentelier_agent_host::{
    AgentHost, HostFailed, MemoryDetail, MemoryEvent, MemoryInspection, MemoryReply, MemoryRequest,
    MemorySource, ReceiveAgentEvents,
};
use agentelier_controls::AgentelierButton;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField};

const GAP: f32 = 8.0;
const MIN_ROWS: usize = 4;
const MAX_ROWS: usize = 12;
const MILLIS_PER_SECOND: i64 = 1_000;

#[derive(Resource, Default)]
pub(super) struct MemoryPane {
    pending: Option<u64>,
    confirming_reset: bool,
    resetting: bool,
    reset_notice: Option<String>,
    inspection: Option<MemoryInspection>,
    detail: Option<MemoryDetail>,
    status: String,
}

#[derive(Clone, Component)]
enum Action {
    BeginReset,
    CancelReset,
    ConfirmReset,
    Refresh,
    Detail { publication: String, record: String },
}

pub(super) fn install(app: &mut App) {
    crate::memory_chat::install(app);
    app.init_resource::<MemoryPane>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

pub(super) fn open(world: &mut World) {
    crate::memory_chat::open(world);
    request(world, Action::Refresh);
}

pub(super) fn close(world: &mut World) {
    let mut state = world.resource_mut::<MemoryPane>();
    state.confirming_reset = false;
    state.inspection = None;
    state.detail = None;
    if !state.resetting {
        state.pending = None;
        state.status.clear();
    }
}

fn request(world: &mut World, action: Action) {
    let mut state = world.resource_mut::<MemoryPane>();
    if state.resetting {
        return;
    }
    let (request, waiting, resetting) = match action {
        Action::BeginReset => {
            state.confirming_reset = true;
            state.reset_notice = None;
            return;
        }
        Action::CancelReset => {
            state.confirming_reset = false;
            return;
        }
        Action::ConfirmReset if state.confirming_reset => {
            state.confirming_reset = false;
            (MemoryRequest::Reset, "正在清空长期记忆…", true)
        }
        Action::ConfirmReset => return,
        Action::Refresh => (MemoryRequest::Inspect, "正在读取长期记忆…", false),
        Action::Detail {
            publication,
            record,
        } => (
            MemoryRequest::Detail {
                publication,
                record,
            },
            "正在读取来源详情…",
            false,
        ),
    };
    let result = world.resource::<AgentHost>().request_memory(request);
    let mut state = world.resource_mut::<MemoryPane>();
    state.detail = None;
    match result {
        Ok(id) => {
            state.pending = Some(id);
            state.resetting = resetting;
            state.status = waiting.into();
            if resetting {
                state.inspection = None;
            }
        }
        Err(failure) => {
            state.pending = None;
            state.status = format!("无法请求长期记忆：{failure}");
        }
    }
}

fn receive(
    mut events: MessageReader<MemoryEvent>,
    mut failures: MessageReader<HostFailed>,
    mut commands: Commands,
) {
    let failure = failures.read().last().map(|event| event.reason.clone());
    let results: Vec<_> = events
        .read()
        .filter(|event| !event.progress && !matches!(event.result, Ok(MemoryReply::Chat(_))))
        .map(|event| (event.operation_id, event.result.clone()))
        .collect();
    if results.is_empty() && failure.is_none() {
        return;
    }
    commands.queue(move |world: &mut World| -> Result {
        let visible = crate::prompts::memory_visible(world);
        if !visible && !world.resource::<MemoryPane>().resetting {
            return Ok(());
        }
        let mut changed = false;
        let mut state = world.resource_mut::<MemoryPane>();
        for (id, result) in results {
            if state.pending != Some(id) {
                continue;
            }
            let resetting = state.resetting;
            state.pending = None;
            state.resetting = false;
            state.status.clear();
            match result {
                Ok(MemoryReply::Reset) => {
                    state.inspection = Some(MemoryInspection {
                        summary: None,
                        sources: Vec::new(),
                        processing: false,
                    });
                    state.reset_notice = Some("长期记忆已清空。".into());
                }
                Ok(MemoryReply::Inspection(inspection)) => {
                    state.inspection = Some(inspection);
                    state.detail = None;
                }
                Ok(MemoryReply::Detail(detail)) => state.detail = Some(detail),
                Ok(MemoryReply::Chat(_)) => continue,
                Err(failure) if resetting => state.reset_notice = Some(failure),
                Err(failure) => state.status = failure,
            }
            changed = true;
        }
        if let Some(failure) = failure
            && state.pending.take().is_some()
        {
            if state.resetting {
                state.resetting = false;
                state.status.clear();
                state.reset_notice = Some("执行连接结束，长期记忆重置结果尚未确认。".into());
            } else {
                state.status = failure;
            }
            changed = true;
        }
        if changed && visible {
            crate::prompts::render(world)?;
        }
        Ok(())
    });
}

fn activate(event: On<Activate>, actions: Query<&Action>, mut commands: Commands) -> Result {
    let action = actions.get(event.entity)?.clone();
    commands.queue(move |world: &mut World| -> Result {
        if crate::prompts::memory_visible(world) {
            request(world, action);
            crate::prompts::render(world)?;
        }
        Ok(())
    });
    Ok(())
}

fn button(label: String, name: impl Into<String>, action: Action, disabled: bool) -> impl Scene {
    let name = name.into();
    (
        bsn! {
            @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) }) as Box<dyn SceneList>)} }
            Name(name) action on(activate)
            Node { align_self: AlignSelf::Start, flex_shrink: 0.0 }
        },
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

fn readonly(text: String, name: &'static str) -> impl Scene {
    bsn! {
        @WidgetryReadOnlyTextField {
            @value: text, @multiline: true,
            @auto_size: {Some(AutoSize { min_rows: MIN_ROWS, max_rows: MAX_ROWS })},
        }
        Name(name) Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
    }
}

fn timestamp(ms: i64) -> Option<String> {
    let time = time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(MILLIS_PER_SECOND)).ok()?;
    Some(format!(
        "{} {:02}:{:02} UTC",
        time.date(),
        time.hour(),
        time.minute()
    ))
}

fn source_label(source: &MemorySource) -> String {
    let when = source
        .activity_ms
        .and_then(timestamp)
        .unwrap_or_else(|| "时间未知".into());
    let directory = source
        .directory
        .as_deref()
        .map(|path| bevy_widgetry_path_display::display_path(std::path::Path::new(path)));
    format!(
        "{} · {} · {}",
        source.session_id,
        when,
        directory.as_deref().unwrap_or("目录未知")
    )
}

pub(super) fn panel(world: &World) -> impl Scene + use<> {
    let state = world.resource::<MemoryPane>();
    let mut status = state.reset_notice.as_ref().map_or_else(
        || state.status.clone(),
        |notice| {
            if state.status.is_empty() {
                notice.clone()
            } else {
                format!("{notice}\n{}", state.status)
            }
        },
    );
    let mut summary = None;
    let mut sources = Vec::<Box<dyn Scene>>::new();
    if let Some(view) = &state.inspection {
        let publication_status = match &view.summary {
            Some(value) => {
                summary = Some(value.text.clone());
                for (index, source) in view.sources.iter().enumerate() {
                    sources.push(Box::new(button(
                        source_label(source),
                        format!("InspectMemorySource{index}"),
                        Action::Detail {
                            publication: value.id.clone(),
                            record: source.record.clone(),
                        },
                        false,
                    )));
                }
                let when = timestamp(value.published_at_ms).unwrap_or_else(|| "时间未知".into());
                format!("最近发布 {when}")
            }
            None => "尚无已发布的长期记忆。".into(),
        };
        if !status.is_empty() {
            status.push('\n');
        }
        status.push_str(&publication_status);
        if view.processing {
            status.push_str("\n后台正在整理长期记忆…");
        }
    }
    let detail = state.detail.as_ref().map(|detail| {
        let title = source_label(&detail.source);
        let record = detail.text.clone();
        bsn_list! {
            Text(title) WidgetryText Name("MemoryDetailSource")
            TextLayout::linebreak(LineBreak::AnyCharacter)--
            @readonly(record, "MemoryDetailBody")
        }
    });
    let summary = summary.map(|text| bsn_list! { @readonly(text, "PublishedMemoryBody") });
    let confirmation = state.confirming_reset.then_some(bsn_list! {
        Text("清空当前数据目录的长期记忆和修订记录？已有会话与账号会保留，记忆将从以后提交的新内容重新积累。")
        WidgetryText Name("MemoryResetConfirmation") TextLayout::linebreak(LineBreak::AnyCharacter)--
        Node { column_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
        Children [
            @button("确认清空长期记忆".into(), "ConfirmMemoryReset", Action::ConfirmReset, false)--
            @button("取消".into(), "CancelMemoryReset", Action::CancelReset, false)
        ]
    });
    let resetting = state.resetting;
    let confirming = state.confirming_reset;
    bsn! {
        Name("MemoryInspection")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            Node { column_gap: px(GAP), flex_wrap: FlexWrap::Wrap }
            Children [
                @button("刷新长期记忆".into(), "RefreshMemoryInspection", Action::Refresh, resetting)--
                @button("清空长期记忆".into(), "BeginMemoryReset", Action::BeginReset, resetting || confirming)
            ]--
            {confirmation}--
            Text(status) WidgetryText Name("MemoryInspectionStatus")
            TextLayout::linebreak(LineBreak::AnyCharacter)--
            {summary}--
            {detail}--
            {(!sources.is_empty()).then_some(bsn! { Text("已发布来源") WidgetryText })}--
            {sources}--
            @crate::memory_chat::panel(world) Node { flex_grow: 0.0, height: px(340), flex_shrink: 0.0 }
        ]
    }
}
