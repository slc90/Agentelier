use super::{Artifact, Preview, child, set_text};
use crate::conversation::ChatState;
use agentelier_agent_host::{
    AgentHost, ImageAttachmentAction, ImageAttachmentEvent, ImageAttachmentReply,
};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::SessionId;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogFilter, WidgetryFileDialogFilterId,
    WidgetryFileDialogMode, WidgetryFileDialogResult, WidgetryFileDialogResultEvent,
    WidgetryFileDialogWindow,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use std::collections::HashMap;
use std::io;

#[derive(Component, Clone, Default)]
struct ExportButton;

#[derive(Component, Clone, Default)]
struct ExportFeedback;

#[derive(Component, Clone)]
struct ExportDialog {
    card: Entity,
    session: SessionId,
    call_id: String,
    digest: String,
}

#[derive(Component)]
struct ExportPending(u64);

pub(super) fn button() -> impl Scene {
    bsn! {
        @AgentelierButton { @children: bsn_list! { Text("另存为") WidgetryText } }
        ExportButton Name("ExportGeneratedImage")
        Node { display: Display::None, align_self: AlignSelf::Start }
        on(open)
    }
}

pub(super) fn feedback() -> impl Scene {
    bsn! {
        ExportFeedback Name("GeneratedImageExportFeedback") Text::default() WidgetryText
        TextLayout::linebreak(LineBreak::AnyCharacter)
        Node { display: Display::None, width: percent(100), min_width: px(0) }
    }
}

pub(super) fn reset(world: &mut World, card: Entity, available: bool) -> Result {
    let button = child::<ExportButton>(world, card)?;
    world.entity_mut(card).remove::<ExportPending>();
    world.entity_mut(button).remove::<InteractionDisabled>();
    world
        .get_mut::<Node>(button)
        .ok_or_else(|| io::Error::other("图片另存为按钮缺少布局"))?
        .display = if available {
        Display::Flex
    } else {
        Display::None
    };
    update_feedback(world, card, String::new())
}

fn open(
    event: On<Activate>,
    parents: Query<&ChildOf, With<ExportButton>>,
    cards: Query<&Preview, Without<ExportPending>>,
    dialogs: Query<(), With<ExportDialog>>,
    mut commands: Commands,
) -> Result {
    if !dialogs.is_empty() {
        return Ok(());
    }
    let card = parents.get(event.entity)?.parent();
    let Ok(preview) = cards.get(card) else {
        return Ok(());
    };
    let Some(Artifact::Saved { digest, mime, .. }) = &preview.artifact else {
        return Ok(());
    };
    let target = ExportDialog {
        card,
        session: preview.session,
        call_id: preview.call_id.clone(),
        digest: digest.clone(),
    };
    let (format, suffixes) = if mime == "image/jpeg" {
        ("JPEG", vec!["jpg".into(), "jpeg".into()])
    } else {
        ("PNG", vec!["png".into()])
    };
    let filters = vec![WidgetryFileDialogFilter {
        id: WidgetryFileDialogFilterId(format.into()),
        label: format.into(),
        default_extension: suffixes.first().cloned(),
        suffixes,
    }];
    let window = WidgetryFileDialogWindow {
        native: Window {
            title: "图片另存为".into(),
            resolution: (840, 580).into(),
            ..default()
        },
        ..default()
    };
    commands.spawn_scene_with_error_handler(bsn! {
        @WidgetryFileDialog { @mode: WidgetryFileDialogMode::SaveFile, @filters: filters, @allow_different_extension: false, @window: {Some(window)} }
        Name("GeneratedImageExportDialog") template(move |_| Ok(target.clone())) on(chosen)
    });
    Ok(())
}

fn chosen(
    event: On<WidgetryFileDialogResultEvent>,
    targets: Query<&ExportDialog>,
    mut commands: Commands,
) -> Result {
    let target = targets.get(event.entity)?.clone();
    let result = event.result.clone();
    commands.queue(move |world: &mut World| -> Result {
        if current_session(world) != Some(target.session)
            || !world.get::<Preview>(target.card).is_some_and(|preview| {
                preview.session == target.session && preview.call_id == target.call_id
                    && matches!(&preview.artifact, Some(Artifact::Saved { digest, .. }) if digest == &target.digest)
            })
        {
            return Ok(());
        }
        let path = match result {
            WidgetryFileDialogResult::Cancelled => {
                return update_feedback(world, target.card, "已取消另存为".into());
            }
            WidgetryFileDialogResult::SavePath(path) => path,
            _ => return Err(io::Error::other("图片另存为缺少保存路径").into()),
        };
        let request = world.resource::<AgentHost>().request_image_attachment(
            target.session, target.call_id, ImageAttachmentAction::Export(path),
        );
        match request {
            Ok(operation) => {
                let button = child::<ExportButton>(world, target.card)?;
                world.entity_mut(target.card).insert(ExportPending(operation));
                world.entity_mut(button).insert(InteractionDisabled);
                update_feedback(world, target.card, "正在保存原图…".into())
            }
            Err(failure) => update_feedback(world, target.card, format!("原图保存失败：{failure}")),
        }
    });
    Ok(())
}

pub(crate) fn receive(
    world: &mut World,
    mut cursor: Local<MessageCursor<ImageAttachmentEvent>>,
) -> Result {
    let session = current_session(world);
    let mut query = world.query::<(Entity, &Preview, &ExportPending)>();
    let mut pending: HashMap<_, _> = query
        .iter(world)
        .filter(|(_, preview, _)| Some(preview.session) == session)
        .map(|(card, _, ExportPending(operation))| (*operation, card))
        .collect();
    let completed: Vec<_> = cursor
        .read(world.resource::<Messages<ImageAttachmentEvent>>())
        .filter_map(|event| {
            let card = pending.remove(&event.operation_id)?;
            let status = match &event.result {
                Ok(ImageAttachmentReply::Exported(path)) => format!(
                    "原图已保存至 {}",
                    bevy_widgetry_path_display::display_path(path)
                ),
                Ok(ImageAttachmentReply::Preview { .. }) => "原图保存失败：收到预览回执".into(),
                Err(failure) => format!("原图保存失败：{failure}"),
            };
            Some((card, status))
        })
        .collect();
    for (card, status) in completed {
        let button = child::<ExportButton>(world, card)?;
        world.entity_mut(card).remove::<ExportPending>();
        world.entity_mut(button).remove::<InteractionDisabled>();
        update_feedback(world, card, status)?;
    }
    Ok(())
}

fn current_session(world: &World) -> Option<SessionId> {
    world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|session| session.id)
}

fn update_feedback(world: &mut World, card: Entity, text: String) -> Result {
    let feedback = child::<ExportFeedback>(world, card)?;
    let display = if text.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    world
        .get_mut::<Node>(feedback)
        .ok_or_else(|| io::Error::other("图片另存为反馈缺少布局"))?
        .display = display;
    set_text(world, feedback, text)
}
