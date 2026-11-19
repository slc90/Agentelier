use agentelier_agent_host::{
    AgentHost, ConversationEvent, HostFailed, MemoryDetail, MemoryEvent, MemoryInspection,
    MemoryReply, MemoryRequest, MemorySource, ReceiveAgentEvents,
};
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField};

const GAP: f32 = 8.0;
const MIN_ROWS: usize = 4;
const MAX_ROWS: usize = 12;
const MILLIS_PER_SECOND: i64 = 1_000;

#[derive(Resource, Default)]
pub(super) struct MemoryPane {
    pending: Option<u64>,
    inspection: Option<MemoryInspection>,
    detail: Option<MemoryDetail>,
    status: String,
}

#[derive(Clone, Component)]
enum Action {
    Refresh,
    Detail { publication: String, record: String },
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<MemoryPane>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

pub(super) fn open(world: &mut World) {
    request(world, Action::Refresh);
}

pub(super) fn close(world: &mut World) {
    *world.resource_mut::<MemoryPane>() = MemoryPane::default();
}

fn request(world: &mut World, action: Action) {
    let (request, waiting) = match action {
        Action::Refresh => (MemoryRequest::Inspect, "正在读取长期记忆…"),
        Action::Detail {
            publication,
            record,
        } => (
            MemoryRequest::Detail {
                publication,
                record,
            },
            "正在读取来源详情…",
        ),
    };
    let result = world.resource::<AgentHost>().request_memory(request);
    let mut state = world.resource_mut::<MemoryPane>();
    state.detail = None;
    match result {
        Ok(id) => {
            state.pending = Some(id);
            state.status = waiting.into();
        }
        Err(failure) => {
            state.pending = None;
            state.status = format!("无法请求长期记忆：{failure}");
        }
    }
}

fn receive(
    mut events: MessageReader<MemoryEvent>,
    mut sessions: MessageReader<ConversationEvent>,
    mut failures: MessageReader<HostFailed>,
    mut commands: Commands,
) {
    let switched = sessions
        .read()
        .filter(|event| {
            matches!(
                event.kind,
                agentelier_conversation_bridge::ConversationEvent::Opened(_)
                    | agentelier_conversation_bridge::ConversationEvent::Created(_)
            )
        })
        .count()
        > 0;
    let failure = failures.read().last().map(|event| event.reason.clone());
    let results: Vec<_> = events
        .read()
        .map(|event| (event.operation_id, event.result.clone()))
        .collect();
    if results.is_empty() && !switched && failure.is_none() {
        return;
    }
    commands.queue(move |world: &mut World| -> Result {
        if !crate::prompts::memory_visible(world) {
            return Ok(());
        }
        if switched {
            open(world);
        }
        let mut changed = switched;
        let mut state = world.resource_mut::<MemoryPane>();
        for (id, result) in results {
            if state.pending != Some(id) {
                continue;
            }
            state.pending = None;
            state.status.clear();
            match result {
                Ok(MemoryReply::Inspection(inspection)) => {
                    state.inspection = Some(inspection);
                    state.detail = None;
                }
                Ok(MemoryReply::Detail(detail)) => state.detail = Some(detail),
                Err(failure) => state.status = failure,
            }
            changed = true;
        }
        if let Some(failure) = failure
            && state.pending.take().is_some()
        {
            state.status = failure;
            changed = true;
        }
        if changed {
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

fn button(label: String, name: impl Into<String>, action: Action) -> impl Scene {
    let name = name.into();
    bsn! {
        @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) }) as Box<dyn SceneList>)} }
        Name(name) action on(activate)
        Node { align_self: AlignSelf::Start, flex_shrink: 0.0 }
    }
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

pub(super) fn panel(state: &MemoryPane) -> impl Scene + use<> {
    let mut status = state.status.clone();
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
    bsn! {
        Name("MemoryInspection")
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            @button("刷新长期记忆".into(), "RefreshMemoryInspection", Action::Refresh)--
            Text(status) WidgetryText Name("MemoryInspectionStatus")
            TextLayout::linebreak(LineBreak::AnyCharacter)--
            {summary}--
            {detail}--
            {(!sources.is_empty()).then_some(bsn! { Text("已发布来源") WidgetryText })}--
            {sources}
        ]
    }
}
