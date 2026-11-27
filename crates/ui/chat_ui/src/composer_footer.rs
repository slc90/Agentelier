use crate::conversation::{ChatState, NewChatTarget, SessionDefaultsSource, SessionModelDefaults};
use crate::preparation::ModelState;
use crate::text_overflow::EllipsizedText;
use crate::{ArchiveState, projects::ProjectState};
use agentelier_agent_host::AgentHost;
use agentelier_conversation_bridge::{SessionModelOptions, SessionModelSelection};
use agentelier_login_ui::AccountState;
use bevy::prelude::*;
use bevy::ui::{Checked, InteractionDisabled, UiSystems};
use bevy::ui_widgets::ValueChange;
use bevy_widgetry::combo_box::{WidgetryComboBox, WidgetryComboBoxAppExt, WidgetryComboBoxContent};
use bevy_widgetry::list_view::{WidgetryListItemId, WidgetryListModel, WidgetryListViewRenderer};
use bevy_widgetry::text::{
    WidgetryText, WidgetryTextColorOverrides, WidgetryTextStateColorOverrides,
};
use bevy_widgetry::toggle_button::WidgetryToggleButtonChanged;
use bevy_widgetry_path_display::display_path;
use std::io;

const FOOTER_GAP: f32 = 8.0;
const FAST_PATH_SPACING: f32 = 16.0;
const MODEL_WIDTH: f32 = 166.0;
const EFFORT_WIDTH: f32 = 100.0;
const PATH_COLOR: Color = Color::srgb_u8(43, 98, 80);
const SESSION_COLOR: Color = Color::srgb_u8(75, 73, 115);

#[derive(Clone)]
pub(super) struct PendingModel {
    pub(super) id: u64,
    selection: SessionModelSelection,
}

#[derive(Component, Clone, Copy, Default)]
struct Footer;

#[derive(Component, Clone, Copy)]
enum Caption {
    Path,
    Session,
}

#[derive(Clone, Copy)]
enum Field {
    Model,
    Effort,
}

#[derive(Component, Clone, Copy)]
struct Selector {
    field: Field,
    source: Entity,
}

#[derive(Clone, PartialEq)]
struct Choice {
    value: String,
    label: String,
}

#[derive(Component, Clone, Copy, Default)]
struct Fast;

enum Change {
    Select(Field, String),
    Fast(bool),
}

pub(super) fn install(app: &mut App) -> Result {
    app.register_widgetry_combo_box::<Choice>()
        .map_err(|failure| failure.into_bevy_error())?;
    app.add_systems(PostUpdate, sync.before(UiSystems::Content));
    Ok(())
}

pub(super) fn sources(commands: &mut Commands, parent: Entity) -> [Entity; 2] {
    std::array::from_fn(|_| {
        commands
            .spawn((WidgetryListModel::<Choice>::default(), ChildOf(parent)))
            .id()
    })
}

pub(super) fn scene([model, effort]: [Entity; 2]) -> impl Scene {
    bsn! {
        Footer Name("ChatComposerFooter")
        Node { display: Display::None, column_gap: px(FOOTER_GAP), align_items: AlignItems::Center }
        Children [
            @selector(Field::Model, model)--
            @selector(Field::Effort, effort)--
            Node { align_items: AlignItems::Center, column_gap: px(FOOTER_GAP), flex_shrink: 0.0, margin: UiRect::right(px(FAST_PATH_SPACING)) }
            Children [
                Text("Fast") WidgetryText--
                @agentelier_controls::agentelier_switch()
                Fast Name("ChatFastMode") on(toggle)
            ]--
            @caption(Caption::Path, "ChatProjectPath", PATH_COLOR)--
            @caption(Caption::Session, "ChatSessionTitle", SESSION_COLOR)--
            @super::conversation::stop_button() Node { display: Display::None }
        ]
    }
}

fn selector(field: Field, source: Entity) -> impl Scene {
    let (name, width, placeholder) = match field {
        Field::Model => ("ChatModel", MODEL_WIDTH, "模型"),
        Field::Effort => ("ChatReasoningEffort", EFFORT_WIDTH, "服务默认"),
    };
    bsn! {
        @WidgetryComboBox::<Choice> {
            @source: source,
            @placeholder: {WidgetryComboBoxContent::new(move || bsn_list! { Text(placeholder) WidgetryText })},
            @renderer: {WidgetryListViewRenderer::new(|choice: &Choice| {
                let label = choice.label.clone();
                bsn_list! { Text(label) WidgetryText TextLayout::no_wrap() }
            })},
        }
        Name(name) template(move |_| Ok(Selector { field, source }))
        Node { width: px(width), flex_shrink: 0.0 } on(select)
    }
}

fn caption(caption: Caption, name: &'static str, color: Color) -> impl Scene {
    bsn! {
        Node { flex_grow: 1.0, flex_basis: px(0), min_width: px(0), overflow: Overflow::clip() }
        Children [
            Text::default() TextLayout::no_wrap() EllipsizedText Name(name) Pickable::IGNORE
            template(move |_| Ok(caption))
            @WidgetryText {
                @colors: {WidgetryTextColorOverrides {
                    normal: WidgetryTextStateColorOverrides { foreground: Some(color) },
                    ..default()
                }},
            }
        ]
    }
}

pub(super) fn draft_model(world: &World) -> Result<SessionModelDefaults> {
    let chat = world.resource::<ChatState>();
    let mut draft = match &chat.draft_model {
        Some(draft) => draft.clone(),
        None => (world.resource::<SessionDefaultsSource>().0)()?,
    };
    if let Some(model) = world
        .resource::<ModelState>()
        .models
        .iter()
        .find(|model| model.slug == draft.slug)
    {
        if let Some(effort) = model.compatible_effort(Some(&draft.effort)) {
            draft.effort = effort.to_owned();
        }
        draft.fast &= model.fast;
    }
    Ok(draft)
}

fn selection(world: &World) -> Result<Option<SessionModelSelection>> {
    let state = world.resource::<ChatState>();
    if let Some(session) = &state.session {
        return Ok(Some(state.pending_model.as_ref().map_or_else(
            || SessionModelSelection {
                slug: session.slug.clone(),
                options: SessionModelOptions {
                    effort: session.effort.clone(),
                    fast: session.fast,
                },
            },
            |pending| pending.selection.clone(),
        )));
    }
    if state.draft_target().is_some() {
        let draft = draft_model(world)?;
        return Ok(Some(SessionModelSelection {
            slug: draft.slug,
            options: SessionModelOptions {
                effort: Some(draft.effort),
                fast: draft.fast,
            },
        }));
    }
    Ok(None)
}

fn enabled(chat: &ChatState, account: &AccountState, models: &ModelState) -> bool {
    if !models.can_create_session(account) {
        return false;
    }
    chat.session
        .as_ref()
        .map_or(chat.operation.is_none(), |session| {
            !session.execution_blocked && session.matches_account(account)
        })
}

fn sync(world: &mut World) -> Result {
    let Ok(footer) = world.query_filtered::<Entity, With<Footer>>().single(world) else {
        return Ok(());
    };
    let selection = selection(world)?;
    let display = if selection.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    world
        .get_mut::<Node>(footer)
        .ok_or_else(|| io::Error::other("聊天底栏缺少布局节点"))?
        .map_unchanged(|node| &mut node.display)
        .set_if_neq(display);
    let Some(selection) = selection else {
        return Ok(());
    };

    let chat = world.resource::<ChatState>();
    let catalog = world.resource::<ModelState>();
    let enabled = enabled(chat, world.resource::<AccountState>(), catalog);
    let requires_websocket = chat
        .session
        .as_ref()
        .is_some_and(|session| session.requires_websocket_model);
    let current_model = catalog
        .models
        .iter()
        .find(|model| model.slug == selection.slug);
    let fast_available = current_model.is_some_and(|model| model.fast || selection.options.fast);
    let mut models: Vec<_> = catalog
        .models
        .iter()
        .filter(|model| {
            (!requires_websocket || model.websocket) && !model.supported_reasoning_levels.is_empty()
        })
        .map(|model| Choice {
            value: model.slug.clone(),
            label: model.display_name.clone(),
        })
        .collect();
    if !models.iter().any(|model| model.value == selection.slug) {
        let label = chat
            .session
            .as_ref()
            .filter(|session| session.slug == selection.slug)
            .map_or(&selection.slug, |session| &session.display_name);
        models.push(Choice {
            value: selection.slug.clone(),
            label: label.clone(),
        });
    }
    let mut efforts: Vec<_> = current_model
        .into_iter()
        .flat_map(|model| &model.supported_reasoning_levels)
        .map(|effort| Choice {
            value: effort.clone(),
            label: effort.clone(),
        })
        .collect();
    let has_efforts = !efforts.is_empty();
    if let Some(effort) = &selection.options.effort
        && !efforts.iter().any(|choice| choice.value == *effort)
    {
        efforts.push(Choice {
            value: effort.clone(),
            label: effort.clone(),
        });
    }
    let (path, title) = captions(world, chat)?;
    for (caption, mut text, mut overflow) in world
        .query::<(&Caption, &mut Text, &mut EllipsizedText)>()
        .iter_mut(world)
    {
        let value = match caption {
            Caption::Path => &path,
            Caption::Session => &title,
        };
        if overflow.full() != value {
            overflow.replace(value, &mut text);
        }
    }
    let selectors = world
        .query::<(Entity, &Selector)>()
        .iter(world)
        .map(|(entity, selector)| (entity, *selector))
        .collect::<Vec<_>>();
    for (entity, selector) in selectors {
        let (choices, value, enabled) = match selector.field {
            Field::Model => (&models, Some(selection.slug.as_str()), enabled),
            Field::Effort => (
                &efforts,
                selection.options.effort.as_deref(),
                enabled && has_efforts,
            ),
        };
        sync_selector(world, entity, selector.source, choices, value)?;
        set_enabled(world, entity, enabled);
    }
    if let Ok(entity) = world.query_filtered::<Entity, With<Fast>>().single(world) {
        set_enabled(world, entity, enabled && fast_available);
        if world.get::<Checked>(entity).is_some() != selection.options.fast {
            let mut button = world.entity_mut(entity);
            if selection.options.fast {
                button.insert(Checked);
            } else {
                button.remove::<Checked>();
            }
        }
    }
    Ok(())
}

fn captions(world: &World, chat: &ChatState) -> Result<(String, String)> {
    let projects = world.resource::<ProjectState>();
    if let Some(session) = &chat.session {
        let path = projects.session_project(session.id).map_or_else(
            || session.working_directory.clone(),
            |project| display_path(&project.directory),
        );
        let title = world
            .resource::<ArchiveState>()
            .entries
            .iter()
            .find(|entry| entry.id == session.id)
            .map(|entry| entry.title.clone())
            .unwrap_or_default();
        return Ok((path, title));
    }
    let path = match chat.draft_target() {
        Some(NewChatTarget::Project(id)) => projects
            .project(id)
            .map(|project| display_path(&project.directory)),
        Some(NewChatTarget::Standalone) => Some(display_path(&std::env::current_dir()?)),
        None => None,
    };
    Ok((path.unwrap_or_default(), String::new()))
}

fn sync_selector(
    world: &mut World,
    entity: Entity,
    source: Entity,
    choices: &[Choice],
    value: Option<&str>,
) -> Result {
    let selected = WidgetryComboBox::<Choice>::selected_id(world, entity)
        .map_err(|failure| failure.into_bevy_error())?;
    let desired = {
        let mut list = world
            .get_mut::<WidgetryListModel<Choice>>(source)
            .ok_or_else(|| io::Error::other("聊天下拉框缺少选项"))?;
        if list.len() != choices.len()
            || choices
                .iter()
                .enumerate()
                .any(|(index, choice)| list.get(index) != Some(choice))
        {
            list.clear();
            for choice in choices {
                list.push(choice.clone())
                    .map_err(|failure| failure.into_bevy_error())?;
            }
        }
        choices
            .iter()
            .position(|choice| Some(choice.value.as_str()) == value)
            .and_then(|index| list.id(index))
    };
    if selected != desired {
        if let Some(id) = desired {
            WidgetryComboBox::<Choice>::select_id(&mut world.commands(), entity, id);
        } else {
            WidgetryComboBox::<Choice>::clear_selection(&mut world.commands(), entity);
        }
    }
    Ok(())
}

fn set_enabled(world: &mut World, entity: Entity, enabled: bool) {
    if enabled == world.get::<InteractionDisabled>(entity).is_some() {
        let mut entity = world.entity_mut(entity);
        if enabled {
            entity.remove::<InteractionDisabled>();
        } else {
            entity.insert(InteractionDisabled);
        }
    }
}

fn select(
    event: On<ValueChange<Option<WidgetryListItemId>>>,
    fields: Query<&Selector>,
    choices: Query<&WidgetryListModel<Choice>>,
    mut commands: Commands,
) {
    if let Ok(selector) = fields.get(event.source)
        && let Some(choice) = event
            .value
            .and_then(|id| choices.get(selector.source).ok()?.get_by_id(id))
    {
        let change = Change::Select(selector.field, choice.value.clone());
        commands.queue(move |world: &mut World| apply(world, change));
    }
}

fn toggle(event: On<WidgetryToggleButtonChanged>, mut commands: Commands) {
    let selected = event.selected;
    commands.queue(move |world: &mut World| apply(world, Change::Fast(selected)));
}

fn apply(world: &mut World, change: Change) -> Result {
    let chat = world.resource::<ChatState>();
    let catalog = world.resource::<ModelState>();
    if !enabled(chat, world.resource::<AccountState>(), catalog) {
        return Ok(());
    }
    let Some(current) = selection(world)? else {
        return Ok(());
    };
    let mut next = current.clone();
    let slug = match &change {
        Change::Select(Field::Model, slug) => slug,
        _ => &current.slug,
    };
    let Some(model) = catalog.models.iter().find(|model| model.slug == *slug) else {
        return Ok(());
    };
    match change {
        Change::Select(Field::Model, slug) if slug != current.slug => {
            if chat
                .session
                .as_ref()
                .is_some_and(|session| session.requires_websocket_model)
                && !model.websocket
            {
                return Ok(());
            }
            let Some(effort) = model.compatible_effort(current.options.effort.as_deref()) else {
                return Ok(());
            };
            next.slug = slug;
            next.options.effort = Some(effort.to_owned());
            next.options.fast &= model.fast;
        }
        Change::Select(Field::Effort, effort)
            if model.supported_reasoning_levels.contains(&effort) =>
        {
            next.options.effort = Some(effort);
        }
        Change::Fast(fast) if model.fast || !fast => next.options.fast = fast,
        _ => {}
    }
    if next == current {
        return Ok(());
    }
    if let Some(session) = &chat.session {
        let submitted = world
            .resource::<AgentHost>()
            .change_session_model(session.id, next.clone());
        let mut state = world.resource_mut::<ChatState>();
        match submitted {
            Ok(id) => {
                state.pending_model = Some(PendingModel {
                    id,
                    selection: next,
                });
                state.input_feedback = None;
            }
            Err(failure) => state.input_feedback = Some(failure.to_string()),
        }
    } else if let Some(effort) = next.options.effort {
        world.resource_mut::<ChatState>().draft_model = Some(SessionModelDefaults {
            slug: next.slug,
            effort,
            fast: next.options.fast,
        });
    }
    Ok(())
}
