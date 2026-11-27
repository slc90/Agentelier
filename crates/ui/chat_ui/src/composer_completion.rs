use crate::{
    composer::{self, Anchor, TextSegment},
    conversation::ChatState,
};
use agentelier_agent_host::{
    AgentHost, ConnectionState, ConnectionStatus, McpStatusEvent, ReceiveAgentEvents, SkillEntry,
    SkillRequest, SkillsEvent,
};
use agentelier_controls::{AgentelierButton, AgentelierButtonStyle};
use agentelier_conversation_bridge::{ComposerContent, McpCapability, SkillIdentity};
use bevy::picking::{events::PointerPress, pointer::PointerButton};
use bevy::prelude::*;
use bevy::text::{FontSize, LineBreak};
use bevy::ui::{InteractionDisabled, OverrideClip};
use bevy::ui_widgets::popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide};
use bevy_widgetry::scene::spawn_scene;
use bevy_widgetry::style::z_index;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::WidgetryTextFieldHeadless;
use bevy_widgetry::theme::WidgetryTheme;

const MAX_ROWS: usize = 7;
const WIDTH: f32 = 420.0;

#[derive(Component, Clone, Default)]
struct Popup;

#[derive(Component, Clone, Copy)]
struct Choice(usize);

#[derive(Clone, PartialEq, Eq)]
struct Trigger {
    root: Entity,
    replacement: Anchor,
    query: String,
}

#[derive(Clone, PartialEq, Eq)]
struct Candidate {
    name: String,
    detail: String,
    content: ComposerContent,
    enabled: bool,
}

#[derive(Resource, Default)]
struct Completion {
    skills: Vec<SkillEntry>,
    connections: Vec<ConnectionStatus>,
    pending_skills: Option<u64>,
    failure: Option<String>,
    trigger: Option<Trigger>,
    dismissed: Option<Trigger>,
    choices: Vec<Candidate>,
    selected: Option<usize>,
    dirty: bool,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Completion>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

fn receive(
    mut skill_events: MessageReader<SkillsEvent>,
    mut statuses: MessageReader<McpStatusEvent>,
    mut state: ResMut<Completion>,
) {
    for event in skill_events.read() {
        if state.pending_skills == Some(event.operation_id) {
            state.pending_skills = None;
            state.failure = event.result.as_ref().err().cloned();
            state.dirty = true;
        }
        if let Ok(snapshot) = &event.result {
            state.skills = snapshot.entries.clone();
            state.dirty = true;
        }
    }
    if let Some(event) = statuses.read().last() {
        state.connections = event.0.clone();
        state.dirty = true;
    }
}

fn trigger(world: &mut World) -> Result<Option<Trigger>> {
    let Some(mut position) = composer::anchor(world)? else {
        return Ok(None);
    };
    if !position.span.is_empty() {
        return Ok(None);
    }
    let Some(root) = world
        .query::<(Entity, &TextSegment)>()
        .iter(world)
        .find_map(|(entity, segment)| (segment.0 == position.segment).then_some(entity))
    else {
        return Ok(None);
    };
    let value = WidgetryTextFieldHeadless::text(world, root)
        .map_err(|failure| failure.into_bevy_error())?;
    let Some(before) = value.get(..position.span.start) else {
        return Ok(None);
    };
    let query = before.trim_end_matches(|ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '-'));
    let Some(prefix) = query.strip_suffix('$') else {
        return Ok(None);
    };
    if prefix.ends_with(|ch: char| ch.is_alphanumeric() || matches!(ch, '\\' | '$')) {
        return Ok(None);
    }
    position.span.start = prefix.len();
    Ok(Some(Trigger {
        root,
        replacement: position,
        query: before[query.len()..].to_lowercase(),
    }))
}

fn live_tools(connections: &[ConnectionStatus]) -> Vec<McpCapability> {
    connections
        .iter()
        .filter(|status| matches!(status.state, ConnectionState::Ready(_)))
        .flat_map(|status| &status.tool_definitions)
        .filter_map(McpCapability::from_definition)
        .collect()
}

fn candidates(state: &Completion, chat: &ChatState, filter: &str) -> Vec<Candidate> {
    let mut skills = state
        .skills
        .iter()
        .map(|skill| {
            let name = skill.metadata.name.clone();
            let source = match &skill.identity {
                SkillIdentity::File(path) => bevy_widgetry_path_display::display_path(path),
                SkillIdentity::Embedded(_) => "内置".into(),
            };
            Candidate {
                name: format!("Skill · {name}"),
                detail: format!("{} · {source}", skill.metadata.description),
                content: ComposerContent::Skill {
                    name,
                    identity: skill.identity.clone(),
                },
                enabled: skill.enabled,
            }
        })
        .collect::<Vec<_>>();
    let mut servers = Vec::<Candidate>::new();
    let mut tools = Vec::new();
    let available = live_tools(&state.connections);
    let frozen = chat
        .session
        .as_ref()
        .map_or(available.as_slice(), |session| &session.mcp_capabilities);
    for tool in frozen {
        let enabled = available
            .iter()
            .any(|candidate| candidate.identity == tool.identity);
        let mut server = tool.identity.clone();
        server.tool = None;
        if !servers.iter().any(|candidate| matches!(&candidate.content, ComposerContent::Mcp { identity, .. } if *identity == server)) {
            let connected = available.iter().any(|candidate| candidate.identity.server == server.server && candidate.identity.target == server.target);
            servers.push(Candidate {
                name: format!("MCP Server · {}", tool.server_name),
                detail: if connected { "已连接" } else { "当前不可用" }.into(),
                content: ComposerContent::Mcp { label: tool.server_name.clone(), identity: server },
                enabled: connected,
            });
        }
        tools.push(Candidate {
            name: format!("MCP Tool · {}", tool.label),
            detail: format!(
                "{} · {} · {}",
                tool.server_name,
                if enabled {
                    "已连接"
                } else {
                    "当前不可用"
                },
                tool.description
            ),
            content: ComposerContent::Mcp {
                label: format!("{}/{}", tool.server_name, tool.label),
                identity: tool.identity.clone(),
            },
            enabled,
        });
    }
    let mut groups = [&mut skills, &mut servers, &mut tools].map(|items| {
        items.retain(|item| {
            item.name.to_lowercase().contains(filter) || item.detail.to_lowercase().contains(filter)
        });
        std::mem::take(items).into_iter()
    });
    let mut result = Vec::new();
    for _ in 0..MAX_ROWS {
        let before = result.len();
        for group in &mut groups {
            if let Some(item) = group.next() {
                result.push(item);
            }
            if result.len() == MAX_ROWS {
                return result;
            }
        }
        if result.len() == before {
            break;
        }
    }
    result
}

pub(crate) fn active(world: &World, root: Entity) -> bool {
    world
        .resource::<Completion>()
        .trigger
        .as_ref()
        .is_some_and(|trigger| trigger.root == root)
}

pub(crate) fn update(world: &mut World) -> Result {
    let next = trigger(world)?;
    let mut state = world.resource_mut::<Completion>();
    if state.dismissed.is_some() && state.dismissed != next {
        state.dismissed = None;
    }
    let next = next.filter(|next| state.dismissed.as_ref() != Some(next));
    if next == state.trigger && !state.dirty {
        return Ok(());
    }
    let request = next.is_some() && state.trigger.is_none() && state.pending_skills.is_none();
    let changed = next != state.trigger;
    state.trigger = next;
    state.dirty = false;
    if request {
        let result = world
            .resource::<AgentHost>()
            .manage_skills(SkillRequest::List);
        let mut state = world.resource_mut::<Completion>();
        match result {
            Ok(id) => {
                state.pending_skills = Some(id);
                state.failure = None;
            }
            Err(failure) => state.failure = Some(failure.to_string()),
        }
    }
    let state = world.resource::<Completion>();
    let choices = state
        .trigger
        .as_ref()
        .map(|trigger| candidates(state, world.resource::<ChatState>(), &trigger.query))
        .unwrap_or_default();
    let mut state = world.resource_mut::<Completion>();
    let selected = if changed {
        choices.iter().position(|candidate| candidate.enabled)
    } else {
        state
            .selected
            .filter(|&index| {
                choices
                    .get(index)
                    .is_some_and(|candidate| candidate.enabled)
            })
            .or_else(|| choices.iter().position(|candidate| candidate.enabled))
    };
    state.choices = choices;
    state.selected = selected;
    render(world)
}

pub(crate) fn key(world: &mut World, key: KeyCode) -> Result {
    let mut state = world.resource_mut::<Completion>();
    if state.trigger.is_none() {
        return Ok(());
    }
    match key {
        KeyCode::Escape => {
            state.dismissed = state.trigger.take();
        }
        KeyCode::ArrowDown | KeyCode::ArrowUp => {
            let selectable = state
                .choices
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| candidate.enabled.then_some(index))
                .collect::<Vec<_>>();
            if let Some(position) = selectable
                .iter()
                .position(|&index| Some(index) == state.selected)
            {
                let destination = if key == KeyCode::ArrowDown {
                    (position + 1) % selectable.len()
                } else {
                    (position + selectable.len() - 1) % selectable.len()
                };
                state.selected = selectable.get(destination).copied();
            }
        }
        KeyCode::Enter | KeyCode::NumpadEnter => {
            if let Some(index) = state.selected {
                return choose(world, index);
            }
        }
        _ => {}
    }
    render(world)
}

fn choose(world: &mut World, index: usize) -> Result {
    let state = world.resource::<Completion>();
    let Some(candidate) = state
        .choices
        .get(index)
        .filter(|candidate| candidate.enabled)
    else {
        return Ok(());
    };
    let Some(trigger) = state.trigger.clone() else {
        return Ok(());
    };
    let content = candidate.content.clone();
    world.resource_mut::<Completion>().trigger = None;
    composer::insert(world, trigger.replacement, content)
}

fn activate(event: On<PointerPress>, choices: Query<&Choice>, mut commands: Commands) -> Result {
    if event.button != PointerButton::Primary {
        return Ok(());
    }
    let index = choices.get(event.entity)?.0;
    commands.queue(move |world: &mut World| choose(world, index));
    Ok(())
}

fn render(world: &mut World) -> Result {
    let old = world
        .query_filtered::<Entity, With<Popup>>()
        .iter(world)
        .collect::<Vec<_>>();
    for entity in old {
        world.despawn(entity);
    }
    let state = world.resource::<Completion>();
    let Some(trigger) = &state.trigger else {
        return Ok(());
    };
    let root = trigger.root;
    if world.get_entity(root).is_err() {
        return Ok(());
    }
    let choices = state.choices.clone();
    let selected = state.selected;
    let empty = if state.pending_skills.is_some() {
        "正在读取能力目录…".to_owned()
    } else {
        state
            .failure
            .clone()
            .unwrap_or_else(|| "没有匹配的能力".into())
    };
    let theme = world.resource::<WidgetryTheme>();
    let background = theme.window.frame.normal.background;
    let border = theme.window.frame.normal.border;
    let popup = spawn_scene(world, bsn! {
        Popup Name("ComposerCompletion") ChildOf(root) OverrideClip GlobalZIndex(z_index::POPUP)
        Popover {
            positions: {vec![
                PopoverPlacement { side: PopoverSide::Top, align: PopoverAlign::Start, gap: 4.0 },
                PopoverPlacement { side: PopoverSide::Bottom, align: PopoverAlign::Start, gap: 4.0 },
            ]},
            window_margin: 8.0,
        }
        BackgroundColor(background) BorderColor::all(border)
        Node { position_type: PositionType::Absolute, width: px(WIDTH), max_width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(3), padding: UiRect::all(px(4)), border: UiRect::all(px(1)), border_radius: BorderRadius::all(px(6)) }
    }).map_err(|failure| failure.into_bevy_error())?;
    if choices.is_empty() {
        spawn_scene(world, bsn! { ChildOf(popup) Text(empty) WidgetryText })
            .map_err(|failure| failure.into_bevy_error())?;
    }
    for (index, candidate) in choices.into_iter().enumerate() {
        let name = candidate.name;
        let detail: String = candidate.detail.chars().take(160).collect();
        let style = if Some(index) == selected {
            AgentelierButtonStyle::Primary
        } else {
            AgentelierButtonStyle::Secondary
        };
        let choice = spawn_scene(world, bsn! {
            @AgentelierButton {
                @style: style,
                @children: bsn_list! {
                    Node { width: percent(100), flex_direction: FlexDirection::Column }
                    Children [
                        Text(name) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
                        Text(detail) WidgetryText TextFont {font_size: FontSize::Px(15.0)} TextLayout::linebreak(LineBreak::AnyCharacter)
                    ]
                }
            }
            ChildOf(popup) Name("ComposerCompletionChoice") template(move |_| Ok(Choice(index))) on(activate)
        }).map_err(|failure| failure.into_bevy_error())?;
        if !candidate.enabled {
            world.entity_mut(choice).insert(InteractionDisabled);
        }
    }
    Ok(())
}
