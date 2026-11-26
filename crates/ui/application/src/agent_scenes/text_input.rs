use super::MAX_LABEL_BYTES;
use agentelier_agent_host::{AgentHostPlugin, EcsError};
use bevy::{
    input::keyboard::{Key, KeyboardInput},
    input_focus::{FocusedInput, tab_navigation::TabIndex},
    prelude::*,
    text::{EditableText, EditableTextFilter, EditableTextSystems},
    ui::{InteractionDisabled, UiSystems},
    ui_widgets::ValueChange,
    window::Ime,
};
use bevy_widgetry::{
    scene::spawn_scene,
    text::WidgetryText,
    text_field::{WidgetryTextField, WidgetryTextFieldHeadless},
    theme::WidgetryTheme,
};
use serde::{Deserialize, Serialize};
use std::io;

const DEFAULT_MAX_LENGTH: usize = 512;
const MAX_LENGTH: usize = 4096;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Properties {
    label: String,
    value: Option<String>,
    placeholder: Option<String>,
    max_length: Option<usize>,
}

#[derive(Component, Clone)]
struct TextInput {
    reported: String,
    placeholder: Option<Entity>,
    max_length: usize,
}

#[derive(Component, Clone, Default)]
struct Placeholder;

#[derive(Component)]
struct SubmitRequested;

pub(crate) type TextChange = ValueChange<String>;

pub(super) fn register(host: AgentHostPlugin) -> Result<AgentHostPlugin> {
    host.with_ecs_scene_factory(
        "agentelier.text_input",
        "Create an editable single-line text parameter. Enter submits the local value.",
        &[
            ("label", "Required nonblank visible label, at most 256 UTF-8 bytes."),
            ("value", "Initial text, including an empty string. Omit only when updating a mounted text input to keep its current text."),
            ("placeholder", "Optional single-line text displayed when empty, at most 256 UTF-8 bytes."),
            ("max_length", "Maximum Unicode scalar count, from 1 to 4096. Defaults to 512."),
        ],
        build,
        None,
        Some(prepare),
    )
}

pub(super) fn install(app: &mut App) {
    app.register_type::<TextChange>().add_systems(
        PostUpdate,
        publish_text
            .after(EditableTextSystems)
            .before(UiSystems::PostLayout),
    );
}

fn build(
    world: &mut World,
    parent: Entity,
    props: Properties,
) -> std::result::Result<Entity, EcsError> {
    let Properties {
        label,
        value,
        placeholder,
        max_length,
    } = props;
    if label.trim().is_empty() || label.len() > MAX_LABEL_BYTES {
        return Err(invalid("label 必须非空且最多 256 字节"));
    }
    if placeholder
        .as_ref()
        .is_some_and(|hint| hint.len() > MAX_LABEL_BYTES || !hint.chars().all(single_line))
    {
        return Err(invalid("placeholder 必须为单行且最多 256 字节"));
    }
    let max_length = max_length.unwrap_or(DEFAULT_MAX_LENGTH);
    if !(1..=MAX_LENGTH).contains(&max_length) {
        return Err(invalid("max_length 必须介于 1 和 4096 之间"));
    }
    let value = value.ok_or_else(|| invalid("创建或更新未挂载的文本输入时必须提供 value"))?;
    if !value.chars().all(single_line) {
        return Err(invalid("value 必须为单行文本"));
    }
    if value.chars().take(max_length + 1).count() > max_length {
        return Err(invalid(format!("value 超过 max_length {max_length}")));
    }
    let initial = value;
    let placeholder = placeholder.map(|hint| {
        let visibility = if initial.is_empty() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        Box::new(bsn_list! {
            Text(hint) Placeholder Pickable::IGNORE TextLayout::no_wrap()
            template(|context| Ok(TextColor(context.resource::<WidgetryTheme>().text.disabled.foreground)))
            template(move |_| Ok(visibility))
            Node { position_type: PositionType::Absolute, left: px(10), right: px(10), align_self: AlignSelf::Center, overflow: Overflow::clip() }
        }) as Box<dyn SceneList>
    });
    let control = bsn! {
        @WidgetryTextField {
            @value: {initial.clone()},
        }
        Name("NativeTextInput") TabIndex(0)
        Node { width: percent(100), min_width: px(0) }
        template(move |_| Ok(TextInput { reported: initial.clone(), placeholder: None, max_length }))
        on(request_submit)
    };
    let root = spawn_scene(world, bsn! {
        ChildOf(parent)
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [
            Text(label) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::AnyCharacter)--
            Node { width: percent(100), min_width: px(0), max_width: px(480), align_items: AlignItems::Center }
            Children [@control-- {placeholder}]
        ]
    }).map_err(construction)?;
    if let Err(error) = initialize(world, root, max_length) {
        world.despawn(root);
        return Err(error);
    }
    Ok(root)
}

fn initialize(
    world: &mut World,
    root: Entity,
    max_length: usize,
) -> std::result::Result<(), EcsError> {
    let control = control(world, root).map_err(construction)?;
    let editor_id =
        WidgetryTextFieldHeadless::editor_entity(world, control).map_err(construction)?;
    // Widgetry's grapheme limiter and Bevy's input filter are mutually exclusive.
    world
        .get_mut::<EditableText>(editor_id)
        .ok_or_else(|| construction("原生文本场景缺少文本缓冲区"))?
        .max_characters = Some(max_length);
    world
        .entity_mut(editor_id)
        .insert(EditableTextFilter::new(single_line));
    let placeholder = world
        .get::<ChildOf>(control)
        .and_then(|parent| world.get::<Children>(parent.parent()))
        .and_then(|children| {
            children
                .iter()
                .find(|entity| world.get::<Placeholder>(*entity).is_some())
        });
    if let Some(mut input) = world.get_mut::<TextInput>(control) {
        input.placeholder = placeholder;
    }
    Ok(())
}

fn prepare(world: &World, root: Entity, mut props: Properties) -> Result<Properties> {
    if props.value.is_none() {
        props.value = Some(
            WidgetryTextFieldHeadless::text(world, control(world, root)?)
                .map_err(|error| error.into_bevy_error())?,
        );
    }
    Ok(props)
}

pub(super) fn control(world: &World, root: Entity) -> Result<Entity> {
    world
        .get::<Children>(root)
        .into_iter()
        .flatten()
        .flat_map(|entity| {
            world
                .get::<Children>(*entity)
                .into_iter()
                .flatten()
                .copied()
        })
        .find(|entity| world.get::<WidgetryTextFieldHeadless>(*entity).is_some())
        .ok_or_else(|| io::Error::other("原生文本场景缺少编辑控件").into())
}

fn request_submit(
    mut event: On<FocusedInput<KeyboardInput>>,
    fields: Query<&WidgetryTextFieldHeadless, Without<InteractionDisabled>>,
    editors: Query<&EditableText, Without<InteractionDisabled>>,
    keys: Res<ButtonInput<Key>>,
    mut ime: MessageReader<Ime>,
    mut commands: Commands,
) {
    if event.input.logical_key != Key::Enter
        || !event.input.state.is_pressed()
        || event.input.repeat
        || [Key::Shift, Key::Control, Key::Alt, Key::Super]
            .iter()
            .any(|key| keys.pressed(key.clone()))
    {
        return;
    }
    let Ok(field) = fields.get(event.focused_entity) else {
        return;
    };
    if event.original_event_target() != field.editor
        || !editors
            .get(field.editor)
            .is_ok_and(|editor| !editor.is_composing())
    {
        return;
    }
    let window = event.input.window;
    let ime_active = ime.read().any(|event| match event {
        Ime::Commit { window: target, .. } => *target == window,
        Ime::Preedit {
            window: target,
            value,
            ..
        } => *target == window && !value.is_empty(),
        _ => false,
    });
    if ime_active {
        return;
    }
    event.propagate(false);
    commands
        .entity(event.focused_entity)
        .try_insert(SubmitRequested);
}

fn publish_text(
    mut fields: Query<(
        Entity,
        &WidgetryTextFieldHeadless,
        &mut TextInput,
        Has<SubmitRequested>,
    )>,
    mut editors: Query<&mut EditableText>,
    mut placeholders: Query<&mut Visibility, With<Placeholder>>,
    mut commands: Commands,
) {
    for (entity, field, mut input, submit) in &mut fields {
        let Ok(mut editor) = editors.get_mut(field.editor) else {
            continue;
        };
        // Widgetry clears Bevy's scalar limit when it initializes its editor.
        if editor.max_characters != Some(input.max_length) {
            editor.max_characters = Some(input.max_length);
        }
        if !editor.is_changed() && !submit {
            continue;
        }
        let current = editor.value();
        if let Some(placeholder) = input.placeholder
            && let Ok(mut visibility) = placeholders.get_mut(placeholder)
        {
            visibility.set_if_neq(if current == "" && !editor.is_composing() {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
        if current != input.reported.as_str() {
            let value = current.to_string();
            input.reported.clone_from(&value);
            commands.trigger(TextChange {
                source: entity,
                value,
                is_final: false,
            });
        }
        if submit {
            commands.entity(entity).try_remove::<SubmitRequested>();
            if !editor.is_composing() {
                commands.trigger(TextChange {
                    source: entity,
                    value: input.reported.clone(),
                    is_final: true,
                });
            }
        }
    }
}

pub(super) fn single_line(character: char) -> bool {
    !matches!(
        character,
        '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
    )
}

fn invalid(error: impl Into<String>) -> EcsError {
    EcsError::new("invalid_scene_props", error)
}

fn construction(error: impl std::fmt::Display) -> EcsError {
    EcsError::new("scene_construction_failed", error.to_string())
}
