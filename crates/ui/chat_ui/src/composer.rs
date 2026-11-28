use crate::conversation::ChatState;
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{
    ComposerContent, ComposerElement, ComposerSubmission, SessionId,
};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::{EditableText, FontCx, LayoutCx, LineBreak};
use bevy::ui_widgets::Activate;
use bevy_widgetry::scene::spawn_scene;
use bevy_widgetry::text::WidgetryText;
use bevy_widgetry::text_field::{AutoSize, WidgetryTextField, WidgetryTextFieldHeadless};
use std::{io, ops::Range};

const MIN_ROWS: usize = 2;
const MAX_ROWS: usize = 6;
const ELEMENT_GAP: f32 = 4.0;

#[derive(Component, Clone, Default)]
struct Body;

#[derive(Component, Clone, Copy)]
pub(crate) struct TextSegment(pub SessionId);

#[derive(Component, Clone, Copy)]
struct RemoveElement(SessionId);

#[derive(Clone)]
pub(crate) struct Draft {
    elements: Vec<ComposerElement>,
    owner: SessionId,
    has_staged: bool,
    legacy_audio: Option<SessionId>,
    cursor: Option<Anchor>,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            elements: vec![text_element(String::new())],
            owner: SessionId::new_v4(),
            has_staged: false,
            legacy_audio: None,
            cursor: None,
        }
    }
}

impl Draft {
    pub(crate) fn text(&self) -> String {
        self.elements
            .iter()
            .filter_map(|element| match &element.content {
                ComposerContent::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn has_input(&self) -> bool {
        self.elements.iter().any(|item| match &item.content {
            ComposerContent::Text { text } => !text.trim().is_empty(),
            _ => true,
        })
    }

    pub(crate) fn has_audio(&self) -> bool {
        self.elements
            .iter()
            .any(|item| matches!(item.content, ComposerContent::Audio { .. }))
    }

    pub(crate) fn submission(&self) -> ComposerSubmission {
        ComposerSubmission {
            elements: self.elements.clone(),
            draft_owner: self.has_staged.then_some(self.owner),
            ..default()
        }
    }

    pub(crate) fn owner(&self) -> SessionId {
        self.owner
    }

    pub(crate) fn placement(&self, revision: u64) -> Option<crate::composer_media::Placement> {
        Some(crate::composer_media::Placement {
            owner: self.owner,
            revision,
            anchor: self.cursor.clone().or_else(|| self.last_anchor())?,
        })
    }

    pub(crate) fn insert_assets(
        &mut self,
        anchor: Anchor,
        assets: Vec<ComposerContent>,
    ) -> Result<Anchor> {
        let mut focus = anchor;
        for asset in assets {
            focus = self.insert(focus, ComposerElement::new(asset))?;
            self.has_staged = true;
        }
        Ok(focus)
    }

    fn index(&self, id: SessionId) -> Option<usize> {
        self.elements.iter().position(|element| element.id == id)
    }

    fn last_anchor(&self) -> Option<Anchor> {
        let last = self.elements.last()?;
        let ComposerContent::Text { text } = &last.content else {
            return None;
        };
        Some(Anchor {
            segment: last.id,
            span: text.len()..text.len(),
        })
    }

    fn insert(&mut self, anchor: Anchor, item: ComposerElement) -> Result<Anchor> {
        let index = self
            .index(anchor.segment)
            .ok_or_else(|| io::Error::other("插入位置已变化"))?;
        let ComposerContent::Text { text } = &mut self.elements[index].content else {
            return Err(io::Error::other("草稿缺少文字插入段").into());
        };
        if text.get(anchor.span.clone()).is_none() {
            return Err(io::Error::other("草稿文字插入范围已变化").into());
        }
        let suffix = text.split_off(anchor.span.end);
        text.truncate(anchor.span.start);
        let tail = text_element(suffix);
        let segment = tail.id;
        self.elements.splice(index + 1..index + 1, [item, tail]);
        Ok(Anchor {
            segment,
            span: 0..0,
        })
    }

    fn remove(&mut self, id: SessionId) -> Result<Anchor> {
        let index = self
            .index(id)
            .ok_or_else(|| io::Error::other("草稿内容项已变化"))?;
        if index == 0
            || index + 1 == self.elements.len()
            || matches!(self.elements[index].content, ComposerContent::Text { .. })
        {
            return Err(io::Error::other("草稿无法移除此位置的内容项").into());
        }
        let suffix = self.elements.remove(index + 1);
        self.elements.remove(index);
        let previous = &mut self.elements[index - 1];
        let (ComposerContent::Text { text }, ComposerContent::Text { text: suffix }) =
            (&mut previous.content, suffix.content)
        else {
            return Err(io::Error::other("草稿内容项缺少相邻文字段").into());
        };
        let offset = text.len();
        text.push_str(&suffix);
        Ok(Anchor {
            segment: previous.id,
            span: offset..offset,
        })
    }
}

fn text_element(text: String) -> ComposerElement {
    ComposerElement::new(ComposerContent::Text { text })
}

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Anchor {
    pub segment: SessionId,
    pub span: Range<usize>,
}

pub(crate) fn install(app: &mut App) {
    app.add_observer(|_: On<Add<Body>>, mut commands: Commands| {
        commands.queue(|world: &mut World| rebuild(world, None));
    });
}

pub(crate) fn body() -> impl Scene {
    bsn! {
        Body Name("ChatComposer")
        Node { width: percent(100), min_width: px(0), flex_shrink: 0.0, flex_direction: FlexDirection::Column, row_gap: px(ELEMENT_GAP) }
    }
}

pub(crate) fn rebuild(world: &mut World, focus: Option<Anchor>) -> Result {
    let Some(parent) = world
        .query_filtered::<Entity, With<Body>>()
        .iter(world)
        .next()
    else {
        return Ok(());
    };
    let state = world.resource::<ChatState>();
    let items = state.composer.elements.clone();
    let draft = state.composer.owner;
    let legacy_audio = state.composer.legacy_audio;
    let session = state.session.as_ref().map(|session| session.id);
    world.entity_mut(parent).despawn_children();
    let compact = items.len() > 1;
    for item in items {
        let id = item.id;
        match item.content {
            ComposerContent::Text { text } => {
                let scene = bsn! {
                    @WidgetryTextField {
                        @value: text, @multiline: true,
                        @auto_size: {Some(AutoSize { min_rows: if compact { 1 } else { MIN_ROWS }, max_rows: MAX_ROWS })},
                    }
                    Name("ChatComposerText") ChildOf(parent) template(move |_| Ok(TextSegment(id)))
                    Node { width: percent(100), min_width: px(0), flex_shrink: 0.0 }
                };
                spawn_scene(world, scene).map_err(|failure| failure.into_bevy_error())?;
            }
            content => mount_atom(
                world,
                parent,
                session.filter(|_| legacy_audio == Some(id)),
                draft,
                id,
                content,
            )?,
        }
    }
    if let Some(anchor) = focus {
        focus_at(world, anchor)?;
    }
    Ok(())
}

fn mount_atom(
    world: &mut World,
    parent: Entity,
    session: Option<SessionId>,
    draft: SessionId,
    id: SessionId,
    content: ComposerContent,
) -> Result {
    let label = match &content {
        ComposerContent::Skill { name, .. } => format!("${name}"),
        ComposerContent::Mcp { label, .. } => format!("${label}"),
        ComposerContent::File { reference } => reference.display(),
        ComposerContent::Image { image } => format!("图片 · {}", image.name),
        ComposerContent::Audio { audio } => format!("音频 · {}", audio.name),
        ComposerContent::Text { .. } => return Err(io::Error::other("文字段无须内容项显示").into()),
    };
    let controls = if let ComposerContent::Audio { audio } = &content {
        let view = agentelier_conversation_bridge::AudioAttachmentView::from(audio.clone());
        let owner = session.map_or(
            agentelier_agent_host::ComposerAssetOwner::Draft(draft),
            agentelier_agent_host::ComposerAssetOwner::Session,
        );
        let audio = crate::audio_attachment::ChatAudio::from(view);
        Some(
            Box::new(bsn_list! { @crate::audio_playback::controls_for(owner, &audio) })
                as Box<dyn SceneList>,
        )
    } else {
        None
    };
    let preview = if let ComposerContent::Image { image } = content {
        Some(Box::new(
            bsn_list! { @crate::composer_media::image(agentelier_agent_host::ComposerAssetOwner::Draft(draft), image) },
        ) as Box<dyn SceneList>)
    } else {
        None
    };
    let scene = bsn! {
        Name("ChatComposerElement") ChildOf(parent)
        Node { column_gap: px(ELEMENT_GAP), align_items: AlignItems::Center, flex_wrap: FlexWrap::Wrap }
        Children [
            Text(label) WidgetryText TextLayout::linebreak(LineBreak::AnyCharacter)--
            {controls}--
            {preview}--
            @AgentelierButton { @children: bsn_list! {Text("移除") WidgetryText} }
            Name("RemoveComposerElement") template(move |_| Ok(RemoveElement(id))) on(remove_clicked)
        ]
    };
    spawn_scene(world, scene).map_err(|failure| failure.into_bevy_error())?;
    Ok(())
}

fn remove_clicked(
    event: On<Activate>,
    items: Query<&RemoveElement>,
    mut commands: Commands,
) -> Result {
    let id = items.get(event.entity)?.0;
    commands.queue(move |world: &mut World| remove(world, id));
    Ok(())
}

pub(crate) fn read_fields(world: &mut World) -> Result {
    let mut fields = world.query::<(Entity, &TextSegment)>();
    let values = fields
        .iter(world)
        .map(|(entity, segment)| {
            WidgetryTextFieldHeadless::text(world, entity).map(|text| (segment.0, text))
        })
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|failure| failure.into_bevy_error())?;
    let mut state = world.resource_mut::<ChatState>();
    let mut changed = false;
    for (id, text) in values {
        let Some(index) = state.composer.index(id) else {
            continue;
        };
        if let ComposerContent::Text { text: previous } =
            &mut state.composer.elements[index].content
            && *previous != text
        {
            *previous = text;
            changed = true;
        }
    }
    if changed {
        state.edit_draft()?;
    }
    Ok(())
}

pub(crate) fn anchor(world: &mut World) -> Result<Option<Anchor>> {
    let focus = world.resource::<InputFocus>().get();
    let mut roots = world.query::<(&TextSegment, &WidgetryTextFieldHeadless)>();
    let field = roots
        .iter(world)
        .find(|(_, field)| Some(field.editor) == focus);
    if let Some((segment, field)) = field {
        let editor = world
            .get::<EditableText>(field.editor)
            .ok_or_else(|| io::Error::other("聊天文字段缺少编辑器"))?;
        if !editor.editor().is_composing() {
            return Ok(Some(Anchor {
                segment: segment.0,
                span: editor.editor().raw_selection().text_range(),
            }));
        }
    }
    Ok(None)
}

pub(crate) fn remember_anchor(world: &mut World) -> Result {
    if let Some(position) = anchor(world)? {
        world.resource_mut::<ChatState>().composer.cursor = Some(position);
    }
    Ok(())
}

pub(crate) fn focus_end(world: &mut World) -> Result {
    let focus = world.resource::<ChatState>().composer.last_anchor();
    if let Some(focus) = focus {
        focus_at(world, focus)?;
    }
    Ok(())
}

pub(crate) fn focus_at(world: &mut World, anchor: Anchor) -> Result {
    let editor = world
        .query::<(&TextSegment, &WidgetryTextFieldHeadless)>()
        .iter(world)
        .find(|(segment, _)| segment.0 == anchor.segment)
        .map(|(_, field)| field.editor);
    let Some(editor) = editor else {
        return Ok(());
    };
    world
        .resource_mut::<InputFocus>()
        .set(editor, FocusCause::Navigated);
    world.resource_scope(|world, mut fonts: Mut<FontCx>| -> Result {
        world.resource_scope(|world, mut layout: Mut<LayoutCx>| -> Result {
            let mut text = world
                .get_mut::<EditableText>(editor)
                .ok_or_else(|| io::Error::other("聊天文字段缺少编辑器"))?;
            text.pending_edits.clear();
            text.editor_mut()
                .driver(&mut fonts.context, &mut layout.0)
                .select_byte_range(anchor.span.start, anchor.span.end);
            Ok(())
        })
    })
}

pub(crate) fn insert(world: &mut World, anchor: Anchor, content: ComposerContent) -> Result {
    read_fields(world)?;
    let mut state = world.resource_mut::<ChatState>();
    let focus = state
        .composer
        .insert(anchor, ComposerElement::new(content))?;
    state.edit_draft()?;
    rebuild(world, Some(focus))
}

pub(crate) fn append(world: &mut World, material: &str) -> Result {
    read_fields(world)?;
    let mut state = world.resource_mut::<ChatState>();
    let aggregate = state.composer.text();
    let separator = if aggregate.is_empty() { "" } else { "\n\n" };
    if aggregate.len() + separator.len() + material.len()
        > agentelier_agent_host::MAX_MATERIAL_BYTES
    {
        return Err(io::Error::other("加入材料后草稿将超过 64 KiB").into());
    }
    let Some(item) = state.composer.elements.last_mut() else {
        return Err(io::Error::other("聊天草稿缺少末尾文字段").into());
    };
    let ComposerContent::Text { text } = &mut item.content else {
        return Err(io::Error::other("聊天草稿末尾无法追加文字").into());
    };
    text.push_str(separator);
    text.push_str(material);
    state.edit_draft()?;
    let focus = state.composer.last_anchor();
    rebuild(world, focus)
}

pub(crate) fn synchronize_audio(world: &mut World) -> Result {
    let state = world.resource::<ChatState>();
    let audio = state
        .session
        .as_ref()
        .and_then(|session| session.audio_draft.clone());
    let current = state.composer.legacy_audio;
    let projected = current
        .and_then(|id| state.composer.index(id))
        .and_then(|index| match &state.composer.elements[index].content {
            ComposerContent::Audio { audio } => Some(audio.id),
            _ => None,
        });
    if projected == audio.as_ref().map(|audio| audio.id) {
        return Ok(());
    }
    let focused = state.composer.cursor.clone();
    let mut state = world.resource_mut::<ChatState>();
    let mut focus = if let Some(current) = current {
        state.composer.legacy_audio = None;
        Some(state.composer.remove(current)?)
    } else {
        focused
    };
    if let Some(audio) = audio {
        let position = focus
            .or_else(|| state.composer.last_anchor())
            .ok_or_else(|| io::Error::other("聊天草稿缺少音频插入位置"))?;
        let item = ComposerElement::new(ComposerContent::Audio {
            audio: audio.attachment(),
        });
        let id = item.id;
        focus = Some(state.composer.insert(position, item)?);
        state.composer.legacy_audio = Some(id);
    }
    state.edit_draft()?;
    rebuild(world, focus)
}

fn remove(world: &mut World, id: SessionId) -> Result {
    read_fields(world)?;
    let mut state = world.resource_mut::<ChatState>();
    if state.composer.index(id).is_none() {
        return Ok(());
    }
    if state.composer.legacy_audio == Some(id) {
        return crate::audio_attachment::request_change(world, None);
    }
    let focus = state.composer.remove(id)?;
    state.edit_draft()?;
    rebuild(world, Some(focus))
}

pub(crate) enum Boundary {
    Focus(Anchor),
    Remove(SessionId),
}

pub(crate) fn boundary(world: &World, root: Entity, key: KeyCode) -> Option<Boundary> {
    let draft = &world.resource::<ChatState>().composer;
    let index = draft.index(world.get::<TextSegment>(root)?.0)?;
    let editor = WidgetryTextFieldHeadless::editor(world, root).ok()?;
    let selection = editor.editor().raw_selection();
    if !selection.is_collapsed() {
        return None;
    }
    let cursor = selection.focus().index();
    let forward = match key {
        KeyCode::ArrowLeft | KeyCode::Backspace if cursor == 0 => false,
        KeyCode::ArrowRight | KeyCode::Delete if cursor == editor.editor().raw_text().len() => true,
        _ => return None,
    };
    let (atom, destination) = if forward {
        (
            draft.elements.get(index + 1)?,
            draft.elements.get(index + 2)?,
        )
    } else {
        (
            draft.elements.get(index.checked_sub(1)?)?,
            draft.elements.get(index.checked_sub(2)?)?,
        )
    };
    if matches!(key, KeyCode::Backspace | KeyCode::Delete) {
        return Some(Boundary::Remove(atom.id));
    }
    let ComposerContent::Text { text } = &destination.content else {
        return None;
    };
    let offset = if forward { 0 } else { text.len() };
    Some(Boundary::Focus(Anchor {
        segment: destination.id,
        span: offset..offset,
    }))
}

pub(crate) fn apply_boundary(world: &mut World, input: Boundary) -> Result {
    match input {
        Boundary::Focus(anchor) => focus_at(world, anchor),
        Boundary::Remove(id) => remove(world, id),
    }
}
