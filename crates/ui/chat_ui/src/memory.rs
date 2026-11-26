use agentelier_agent_host::{
    AgentHost, HostFailed, MemoryDetail, MemoryEvent, MemoryInspection, MemoryReply, MemoryRequest,
    MemoryWorkEvent, MemoryWorkState, ReceiveAgentEvents,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use bevy::{
    prelude::*,
    text::{FontSize, LineBreak},
    ui::UiSystems,
    ui_widgets::Activate,
};
use bevy_widgetry::collapsible::{WidgetryCollapsible, WidgetryCollapsibleChanged};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::scroll_area::WidgetryScrollArea;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryReadOnlyTextField, WidgetryTextFieldHeadless};
use bevy_widgetry::window::{WidgetryWindowBackground, owned_widgetry_window};
use std::collections::BTreeSet;

const GAP: f32 = 8.0;

#[derive(Resource, Default)]
struct Overview {
    inspect_operation: Option<u64>,
    refresh_needed: bool,
    inspection: Option<MemoryInspection>,
    work: MemoryWorkState,
    inspect_error: Option<String>,
    detail_operation: Option<u64>,
    detail_label: Option<String>,
    detail: Option<MemoryDetail>,
    detail_error: Option<String>,
    categories_open: BTreeSet<&'static str>,
    body_dirty: bool,
    detail_dirty: bool,
}

#[derive(Component, Default, Clone)]
struct MemoryWindow;
#[derive(Component, Default, Clone)]
struct SummarySlot;
#[derive(Component, Default, Clone)]
struct DetailSlot;
#[derive(Component, Default, Clone)]
struct PublishedAt;
#[derive(Component, Default, Clone)]
struct OverviewStatus;
#[derive(Component)]
struct InitialReadonly(String);
#[derive(Component, Clone)]
struct SourceChoice {
    publication: String,
    record: String,
    label: String,
}
#[derive(Component, Clone)]
struct CategoryChoice(&'static str);

pub(super) fn install(app: &mut App) {
    crate::memory_chat::install(app);
    agentelier_controls::install_owned_window_close::<With<MemoryWindow>>(app);
    app.init_resource::<Overview>()
        .add_systems(
            Update,
            (receive, request_refresh).chain().after(ReceiveAgentEvents),
        )
        .add_systems(
            PostUpdate,
            (project, initialize_readonly)
                .chain()
                .before(UiSystems::Content),
        );
}

pub(super) fn open(world: &mut World) -> Result {
    if world
        .query_filtered::<Entity, With<MemoryWindow>>()
        .iter(world)
        .next()
        .is_some()
    {
        return Ok(());
    }
    let chat = crate::memory_chat::panel(world);
    world.commands().spawn_scene_with_error_handler(bsn! {
        MemoryWindow Name("MemoryManagementWindow")
        @owned_widgetry_window(
            Window { title: "记忆管理".into(), resolution: (1030, 780).into(), resizable: false, ..default() },
            agentelier_controls::MANAGEMENT_WINDOW_CONTROLS,
            WidgetryWindowBackground::Theme, Default::default(),
            bsn_list! { @agentelier_controls::padded_window_title() },
            bsn_list! {
                Node {
                    width: percent(100), min_height: px(0), flex_grow: 1.0,
                    flex_direction: FlexDirection::Column, row_gap: px(GAP), padding: UiRect::all(px(14))
                }
                Children [
                    Node { width: percent(100), align_items: AlignItems::Center, column_gap: px(12) }
                    Children [
                        Text("长期记忆") WidgetryText TextFont { font_size: FontSize::Px(25.0) }--
                        PublishedAt Name("MemoryPublishedAt") Text("") WidgetryText Node { flex_grow: 1.0 }
                    ]--
                    OverviewStatus Name("MemoryInspectionStatus") Text("") WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                    Node { width: percent(100), height: percent(42), min_height: px(0), column_gap: px(12) }
                    Children [
                        @WidgetryScrollArea {
                            @children: bsn_list! {
                                SummarySlot Name("MemorySummarySlot")
                                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
                            }
                        }
                        Name("MemorySummaryScroll") Node { flex_basis: percent(52), min_width: px(0), min_height: px(0) }--
                        @WidgetryScrollArea {
                            @children: bsn_list! {
                                DetailSlot Name("MemoryDetailSlot")
                                Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column }
                            }
                        }
                        Name("MemoryDetailScroll") Node { flex_basis: percent(48), min_width: px(0), min_height: px(0) }
                    ]--
                    @chat
                ]
            },
        )
    });
    let mut overview = world.resource_mut::<Overview>();
    clear_inspection(&mut overview);
    overview.refresh_needed = true;
    crate::memory_chat::open(world);
    Ok(())
}

fn request_refresh(
    windows: Query<(), With<MemoryWindow>>,
    host: Res<AgentHost>,
    mut overview: ResMut<Overview>,
) {
    if windows.is_empty() || !overview.refresh_needed || overview.inspect_operation.is_some() {
        return;
    }
    overview.refresh_needed = false;
    match host.request_memory(MemoryRequest::Inspect) {
        Ok(operation) => overview.inspect_operation = Some(operation),
        Err(failure) => {
            clear_inspection(&mut overview);
            overview.inspect_error = Some(failure.to_string());
        }
    }
}

fn receive(
    mut replies: MessageReader<MemoryEvent>,
    mut work: MessageReader<MemoryWorkEvent>,
    mut failures: MessageReader<HostFailed>,
    mut overview: ResMut<Overview>,
) {
    if let Some(event) = work.read().last() {
        overview.work = event.0;
        overview.refresh_needed = true;
    }
    for event in replies.read().filter(|event| !event.progress) {
        if overview.inspect_operation == Some(event.operation_id) {
            overview.inspect_operation = None;
            match &event.result {
                Ok(MemoryReply::Inspection(inspection)) => {
                    apply_inspection(&mut overview, inspection)
                }
                Err(failure) => {
                    clear_inspection(&mut overview);
                    overview.inspect_error = Some(failure.clone());
                }
                _ => {}
            }
        } else if overview.detail_operation == Some(event.operation_id) {
            overview.detail_operation = None;
            match &event.result {
                Ok(MemoryReply::Detail(detail)) => overview.detail = Some(detail.clone()),
                Err(failure) => overview.detail_error = Some(failure.clone()),
                _ => {}
            }
            overview.detail_dirty = true;
        } else if matches!(&event.result, Ok(MemoryReply::Chat(view)) if view.turn.is_some()) {
            overview.refresh_needed = true;
        }
    }
    for failure in failures.read() {
        overview.inspect_operation = None;
        clear_inspection(&mut overview);
        overview.inspect_error = Some(failure.reason.clone());
    }
}

fn apply_inspection(overview: &mut Overview, inspection: &MemoryInspection) {
    let publication =
        |value: &MemoryInspection| value.summary.as_ref().map(|summary| summary.id.clone());
    let old = overview.inspection.as_ref();
    if old.and_then(publication) != publication(inspection) {
        clear_inspection(overview);
    } else if old.is_none_or(|previous| previous.groups != inspection.groups) {
        overview.body_dirty = true;
    }
    overview.inspect_error = None;
    overview.inspection = Some(inspection.clone());
}

fn clear_inspection(overview: &mut Overview) {
    overview.inspection = None;
    overview.inspect_error = None;
    overview.detail = None;
    overview.detail_label = None;
    overview.detail_error = None;
    overview.detail_operation = None;
    overview.body_dirty = true;
    overview.detail_dirty = true;
}

fn project(world: &mut World) -> Result {
    let Some(slot) = world
        .query_filtered::<Entity, With<SummarySlot>>()
        .iter(world)
        .next()
    else {
        return Ok(());
    };
    let overview = world.resource::<Overview>();
    let published_at = overview
        .inspection
        .as_ref()
        .and_then(|value| value.summary.as_ref())
        .and_then(|value| timestamp(value.published_at_ms))
        .map_or_else(String::new, |date| format!("最近发布 {date}"));
    let status = overview_text(overview);
    if overview.body_dirty {
        let scene = body(overview);
        world.entity_mut(slot).despawn_related::<Children>();
        world
            .commands()
            .spawn_scene_with_error_handler(bsn! { @scene ChildOf(slot) });
        world.resource_mut::<Overview>().body_dirty = false;
    }
    if world.resource::<Overview>().detail_dirty
        && let Some(slot) = world
            .query_filtered::<Entity, With<DetailSlot>>()
            .iter(world)
            .next()
    {
        let scene = detail(world.resource::<Overview>());
        world.entity_mut(slot).despawn_related::<Children>();
        world
            .commands()
            .spawn_scene_with_error_handler(bsn! { @scene ChildOf(slot) });
        world.resource_mut::<Overview>().detail_dirty = false;
    }
    if let Ok(mut text) = world
        .query_filtered::<&mut Text, With<PublishedAt>>()
        .single_mut(world)
    {
        text.set_if_neq(Text(published_at));
    }
    if let Ok(mut text) = world
        .query_filtered::<&mut Text, With<OverviewStatus>>()
        .single_mut(world)
    {
        text.set_if_neq(Text(status));
    }
    Ok(())
}

fn overview_text(overview: &Overview) -> String {
    let mut messages = Vec::new();
    if let Some(error) = &overview.inspect_error {
        messages.push(error.clone());
    }
    if let Some(inspection) = &overview.inspection {
        if let Some(notice) = inspection.notice {
            messages.push(notice.into());
        }
        if inspection.pending_revisions > 0 {
            messages.push(format!(
                "{} 条修订已保存，等待整合。",
                inspection.pending_revisions
            ));
        }
        if inspection.pending_sources > 0 {
            messages.push(format!(
                "{} 个来源等待提取或正在提取。",
                inspection.pending_sources
            ));
        }
        if inspection.failed_sources > 0 {
            messages.push(format!(
                "{} 个来源提取失败，等待重试。",
                inspection.failed_sources
            ));
        }
    } else if overview.inspect_operation.is_some() {
        messages.push("正在读取已发布记忆…".into());
    }
    match overview.work {
        MemoryWorkState::Idle
            if overview
                .inspection
                .as_ref()
                .is_some_and(|value| value.processing) =>
        {
            messages.push("后台正在整理长期记忆…".into());
        }
        MemoryWorkState::Idle => {}
        MemoryWorkState::Working => messages.push("后台正在整理长期记忆…".into()),
        MemoryWorkState::SourceScanFailed => messages.push("扫描长期记忆来源失败。".into()),
        MemoryWorkState::PublicationFailed => messages.push("长期记忆整合或发布失败。".into()),
        MemoryWorkState::WaitingForModel => {
            messages.push("记忆整合正在等待可用账号和模型。".into())
        }
    }
    messages.join("\n")
}

fn body(overview: &Overview) -> impl Scene + use<> {
    let inspection = overview.inspection.as_ref();
    let summary = inspection.and_then(|value| value.summary.as_ref());
    let summary_text = summary.map(|value| value.text.clone());
    let summary_title = "发布摘要";
    let summary_open = overview.categories_open.contains(summary_title);
    let groups = inspection.into_iter().flat_map(|value| &value.groups).enumerate().map(|(group_index, group)| {
        let title = group.title;
        let count = group.records.len();
        let heading = format!("{title} · {count}");
        let expanded = overview.categories_open.contains(title);
        let rows = summary.into_iter().flat_map(|summary| {
            group.records.iter().enumerate().map(move |(index, source)| {
                let when = source.activity_ms.and_then(timestamp).unwrap_or_else(|| "时间未知".into());
                let directory = source.directory.as_deref().map(|path| bevy_widgetry_path_display::display_path(std::path::Path::new(path)));
                let label = format!("{} · {} · {}", source.scope, source.status, when);
                let button_text = if source.preview.is_empty() { label.clone() } else { format!("{}\n{label}", source.preview) };
                let detail_label = format!("{title} · {label}\n{}\n会话 {}\n{}发布 {}", source.provenance, source.session_id,
                    directory.as_ref().map_or_else(String::new, |path| format!("目录 {path}\n")), summary.id);
                let action = SourceChoice { publication: summary.id.clone(), record: source.record.clone(), label: detail_label };
                bsn! {
                    @button(button_text) Name({format!("InspectMemorySource{group_index}-{index}")})
                    template(move |_| Ok(action.clone())) on(inspect_source)
                }
            })
        }).collect::<Vec<_>>();
        bsn! {
            @WidgetryCollapsible {
                @expanded: expanded,
                @header: bsn! { Text(heading) WidgetryText Name({format!("MemoryCategory{group_index}")}) },
                @children: bsn_list! {
                    Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
                    Children [
                        @{(count == 0).then_some(bsn! { Text("暂无记录") WidgetryText })}--
                        {rows}
                    ]
                },
            }
            template(move |_| Ok(CategoryChoice(title))) on(toggle_category)
        }
    }).collect::<Vec<_>>();
    bsn! {
        Name("MemorySummaryContent")
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP), flex_shrink: 0.0 }
        Children [
            {groups}--
            @{summary_text.map(|text| bsn! {
                @WidgetryCollapsible {
                    @expanded: summary_open,
                    @header: bsn! { Text(summary_title) WidgetryText Name("ToggleMemorySummary") },
                    @children: bsn_list! { @readonly(text, "PublishedMemoryBody", 6) },
                }
                template(move |_| Ok(CategoryChoice(summary_title))) on(toggle_category)
            })}
        ]
    }
}

fn detail(overview: &Overview) -> impl Scene + use<> {
    let title = overview.detail_label.clone();
    let text = overview.detail.as_ref().map(|value| value.text.clone());
    let detail_status = if overview.detail_operation.is_some() {
        Some("正在读取来源详情…".into())
    } else {
        overview.detail_error.clone()
    };
    let empty = title.is_none() && detail_status.is_none();
    bsn! {
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(GAP) }
        Children [
            @{title.map(|title| bsn! { Text(title) WidgetryText Name("MemoryDetailSource") TextLayout::linebreak(LineBreak::AnyCharacter) })}--
            @{detail_status.map(|text| bsn! { Text(text) WidgetryText Name("MemoryDetailStatus") TextLayout::linebreak(LineBreak::AnyCharacter) })}--
            @{text.map(|text| readonly(text, "MemoryDetailBody", 9))}--
            @{empty.then_some(bsn! { Text("选择一条记忆查看详情。") WidgetryText })}
        ]
    }
}

fn toggle_category(
    event: On<WidgetryCollapsibleChanged>,
    choices: Query<&CategoryChoice>,
    mut overview: ResMut<Overview>,
) -> Result {
    let choice = choices.get(event.entity)?;
    if event.expanded {
        overview.categories_open.insert(choice.0);
    } else {
        overview.categories_open.remove(choice.0);
    }
    Ok(())
}

fn inspect_source(
    event: On<Activate>,
    choices: Query<&SourceChoice>,
    host: Res<AgentHost>,
    mut overview: ResMut<Overview>,
) -> Result {
    let choice = choices.get(event.entity)?;
    if overview
        .inspection
        .as_ref()
        .and_then(|value| value.summary.as_ref())
        .is_none_or(|summary| summary.id != choice.publication)
    {
        return Ok(());
    }
    overview.detail = None;
    overview.detail_label = Some(choice.label.clone());
    overview.detail_error = None;
    match host.request_memory(MemoryRequest::Detail {
        publication: choice.publication.clone(),
        record: choice.record.clone(),
    }) {
        Ok(operation) => overview.detail_operation = Some(operation),
        Err(failure) => {
            overview.detail_operation = None;
            overview.detail_error = Some(failure.to_string());
        }
    }
    overview.detail_dirty = true;
    Ok(())
}

fn timestamp(ms: i64) -> Option<String> {
    let time = time::OffsetDateTime::from_unix_timestamp(ms.div_euclid(1_000)).ok()?;
    Some(format!(
        "{} {:02}:{:02} UTC",
        time.date(),
        time.hour(),
        time.minute()
    ))
}

fn readonly(text: String, name: &'static str, rows: usize) -> impl Scene {
    bsn! {
        @WidgetryReadOnlyTextField {
            @multiline: true,
            @auto_size: {Some(AutoSize { min_rows: 3, max_rows: rows })},
        }
        template(move |_| Ok(InitialReadonly(text.clone())))
        Name(name) Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
    }
}

fn initialize_readonly(world: &mut World) -> Result {
    let pending: Vec<_> = world
        .query_filtered::<Entity, With<InitialReadonly>>()
        .iter(world)
        .collect();
    for entity in pending {
        if let Some(text) = world.entity_mut(entity).take::<InitialReadonly>() {
            WidgetryTextFieldHeadless::set_text_in_world(world, entity, text.0)
                .map_err(|failure| failure.into_bevy_error())?;
        }
    }
    Ok(())
}

fn button(label: impl Into<String>) -> impl Scene {
    let label = label.into();
    bsn! {
        @AgentelierButton {
            @style: AgentelierButtonStyle::Secondary,
            @children: bsn_list! { Text(label) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter) }
        }
        Node { align_self: AlignSelf::Start, max_width: percent(100), flex_shrink: 0.0 }
    }
}
