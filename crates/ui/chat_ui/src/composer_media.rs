use crate::{composer::Anchor, conversation::ChatState};
use agentelier_agent_host::{
    AgentHost, ComposerAssetOwner, ComposerAssetReply, ComposerAssetRequest, ComposerImageSource,
};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{FileReference, FileScope, InputImage, SessionId};
use bevy::asset::RenderAssetUsages;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::ui_widgets::Activate;
use bevy::window::RequestRedraw;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogFilter, WidgetryFileDialogFilterId,
    WidgetryFileDialogMode, WidgetryFileDialogResult, WidgetryFileDialogResultEvent,
    WidgetryFileDialogWindow,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;
use std::io;

const PREVIEW_EDGE: f32 = 150.0;

#[derive(Clone)]
pub(crate) struct Placement {
    pub owner: SessionId,
    pub revision: u64,
    pub anchor: Anchor,
}

#[derive(Component, Clone)]
struct Picker(Placement);

#[derive(Component, Clone, Default)]
struct AddImage;

#[derive(Component, Clone)]
struct Preview {
    owner: ComposerAssetOwner,
    image: InputImage,
}

#[derive(Component)]
struct Loading(Task<Result<Image, String>>);

#[derive(Component)]
struct Resolved;

#[derive(Resource, Default)]
struct Imports(Vec<(Placement, Task<io::Result<ComposerAssetReply>>)>);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<Imports>()
        .add_systems(Update, (button_visibility, prepare_previews))
        .add_systems(
            PostUpdate,
            complete_imports.after(bevy::text::EditableTextSystems),
        );
}

pub(crate) fn add_button() -> impl Scene {
    bsn! {
        @AgentelierButton { @children: bsn_list! { Text("添加图片") WidgetryText } }
        AddImage Name("AddComposerImage") Node { display: Display::None } on(open_picker)
    }
}

fn button_visibility(chat: Res<ChatState>, mut buttons: Query<&mut Node, With<AddImage>>) {
    let visible = chat.session.is_some() || chat.draft_target().is_some();
    for node in &mut buttons {
        node.map_unchanged(|node| &mut node.display)
            .set_if_neq(if visible {
                Display::Flex
            } else {
                Display::None
            });
    }
}

fn open_picker(_: On<Activate>, chat: Res<ChatState>, mut commands: Commands) {
    let Some(placement) = chat.composer.placement(chat.draft_revision) else {
        return;
    };
    let window = WidgetryFileDialogWindow {
        native: Window {
            title: "选择图片".into(),
            resolution: (840, 580).into(),
            ..default()
        },
        ..default()
    };
    let filters = vec![WidgetryFileDialogFilter {
        id: WidgetryFileDialogFilterId("input-images".into()),
        label: "图片 (PNG, JPEG)".into(),
        suffixes: vec!["png".into(), "jpg".into(), "jpeg".into()],
        default_extension: None,
    }];
    commands.spawn_scene_with_error_handler(bsn! {
        @WidgetryFileDialog { @mode: WidgetryFileDialogMode::PickFiles, @window: {Some(window)}, @filters: filters }
        Name("ComposerImagePicker") template(move |_| Ok(Picker(placement.clone()))) on(picked)
    });
}

fn picked(event: On<WidgetryFileDialogResultEvent>, query: Query<&Picker>, mut commands: Commands) {
    let Ok(picker) = query.get(event.entity) else {
        return;
    };
    let placement = picker.0.clone();
    let paths = match &event.result {
        WidgetryFileDialogResult::Files(paths) => paths.to_vec(),
        WidgetryFileDialogResult::File(path) => vec![path.clone()],
        _ => return,
    };
    commands.queue(move |world: &mut World| {
        stage(
            world,
            placement,
            paths.into_iter().map(ComposerImageSource::Local).collect(),
        )
    });
}

pub(crate) fn select_file(
    world: &mut World,
    reference: FileReference,
    scope: FileScope,
    anchor: Anchor,
) -> Result<bool> {
    if !reference
        .path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| matches!(value.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg"))
    {
        return Ok(false);
    }
    let chat = world.resource::<ChatState>();
    let placement = Placement {
        owner: chat.composer.owner(),
        revision: chat.draft_revision,
        anchor,
    };
    stage(
        world,
        placement,
        vec![ComposerImageSource::Project { reference, scope }],
    )?;
    Ok(true)
}

fn stage(world: &mut World, placement: Placement, sources: Vec<ComposerImageSource>) -> Result {
    let request = ComposerAssetRequest::StageImages {
        draft: placement.owner,
        sources,
    };
    match world.resource::<AgentHost>().composer_asset(request) {
        Ok(reply) => {
            let task = AsyncComputeTaskPool::get().spawn(reply);
            world.resource_mut::<Imports>().0.push((placement, task));
        }
        Err(failure) => {
            world.resource_mut::<ChatState>().input_feedback = Some(failure.to_string())
        }
    }
    Ok(())
}

pub(crate) fn importing(world: &World, owner: SessionId) -> bool {
    world
        .resource::<Imports>()
        .0
        .iter()
        .any(|(placement, _)| placement.owner == owner)
}

fn complete_imports(world: &mut World) -> Result {
    let mut ready = Vec::new();
    world
        .resource_mut::<Imports>()
        .0
        .retain_mut(|(placement, task)| {
            if let Some(result) = check_ready(task) {
                ready.push((placement.clone(), result));
                false
            } else {
                true
            }
        });
    if !world.resource::<Imports>().0.is_empty() {
        world.write_message(RequestRedraw);
    }
    for (placement, result) in ready {
        let mut attachments = Vec::new();
        let mut errors = Vec::new();
        match result {
            Ok(ComposerAssetReply::Staged(items)) => {
                for item in items {
                    match item {
                        Ok(attachment) => attachments.push(attachment),
                        Err(failure) => errors.push(failure.to_string()),
                    }
                }
            }
            Ok(_) => errors.push("收到不匹配的附件回执".into()),
            Err(failure) => errors.push(failure.to_string()),
        }
        crate::conversation::apply_draft_assets(world, placement, attachments, errors)?;
    }
    Ok(())
}

pub(crate) fn image(owner: ComposerAssetOwner, image: InputImage) -> impl Scene {
    let width = image.image.width as f32;
    let height = image.image.height as f32;
    let shrink = (PREVIEW_EDGE / width.max(height)).min(1.0);
    let label = format!("读取图片 · {}", image.name);
    bsn! {
        Name("ComposerImagePreview") template(move |_| Ok(Preview { owner, image: image.clone() }))
        Node { width: px(width * shrink), height: px(height * shrink), flex_shrink: 0.0, max_width: percent(100) }
        Children [Text(label) WidgetryText Pickable::IGNORE]
    }
}

fn prepare_previews(
    mut commands: Commands,
    previews: Query<(Entity, &Preview), Without<Resolved>>,
    mut loading: Query<&mut Loading>,
    host: Res<AgentHost>,
    mut assets: ResMut<Assets<Image>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    for (entity, preview) in &previews {
        if let Ok(mut pending) = loading.get_mut(entity) {
            if let Some(result) = check_ready(&mut pending.0) {
                let mut node = commands.entity(entity);
                node.despawn_children().remove::<Loading>().insert(Resolved);
                match result {
                    Ok(image) => {
                        node.insert(ImageNode::new(assets.add(image)));
                    }
                    Err(failure) => {
                        node.with_child((
                            Text(format!("图片预览失败：{failure}")),
                            WidgetryText::default(),
                        ));
                    }
                }
            } else {
                redraw.write(RequestRedraw);
            }
            continue;
        }
        let request = ComposerAssetRequest::Image {
            owner: preview.owner,
            image: preview.image.clone(),
        };
        let mut node = commands.entity(entity);
        match host.composer_asset(request) {
            Ok(reply) => {
                let task = AsyncComputeTaskPool::get().spawn(async move {
                    let reply = reply.await.map_err(|failure| failure.to_string())?;
                    let ComposerAssetReply::Image(image) = reply else {
                        return Err("收到不匹配的图片回执".into());
                    };
                    Image::from_buffer(
                        image.bytes(),
                        ImageType::MimeType(image.mime()),
                        CompressedImageFormats::NONE,
                        true,
                        ImageSampler::default(),
                        RenderAssetUsages::default(),
                    )
                    .map_err(|failure| failure.to_string())
                });
                node.insert(Loading(task));
                redraw.write(RequestRedraw);
            }
            Err(failure) => {
                node.despawn_children().insert(Resolved).with_child((
                    Text(format!("图片预览失败：{failure}")),
                    WidgetryText::default(),
                ));
            }
        }
    }
}
