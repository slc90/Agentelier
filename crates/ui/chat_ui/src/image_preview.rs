use crate::conversation::ChatToolImage;
use agentelier_agent_host::{
    AgentHost, HostFailed, ImageAttachmentAction, ImageAttachmentEvent, ImageAttachmentReply,
    ReceiveAgentEvents,
};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::SessionId;
use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::ui::InteractionDisabled;
use bevy::ui_widgets::Activate;
use bevy::window::PrimaryWindow;
use bevy_widgetry::file_dialog::{
    WidgetryFileDialog, WidgetryFileDialogFilter, WidgetryFileDialogFilterId,
    WidgetryFileDialogModality, WidgetryFileDialogMode, WidgetryFileDialogResult,
    WidgetryFileDialogResultEvent, WidgetryFileDialogWindow,
};
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::text::WidgetryText;

const PREVIEW_WIDTH: f32 = 320.0;
const PREVIEW_HEIGHT: f32 = 240.0;

#[derive(Component, Clone)]
pub(crate) struct ImageCard {
    session: SessionId,
    call: String,
    extension: &'static str,
    preview: Preview,
    export: Export,
}

#[derive(Clone)]
enum Preview {
    Pending,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Clone)]
enum Export {
    Idle,
    Choosing(Entity),
    Writing(u64),
    Finished(String),
}

#[derive(Component, Clone, Default)]
struct PreviewNode;

#[derive(Component, Clone, Default)]
struct ImageStatus;

#[derive(Component, Clone, Default)]
struct SaveButton;

#[derive(Component, Clone)]
struct SavePicker(Entity);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<PreviewRequest>().add_systems(
        Update,
        (receive, start_preview, check_picker, synchronize)
            .chain()
            .after(ReceiveAgentEvents),
    );
}

pub(crate) fn card(session: SessionId, call: String, image: ChatToolImage) -> impl Scene {
    let (label, extension) = if image.mime == "image/png" {
        ("PNG", "png")
    } else {
        ("JPEG", "jpg")
    };
    let metadata = format!("{label} · {} × {}", image.width, image.height);
    let name = format!("ToolImage-{call}");
    let exportable = image.exportable;
    let width = image.width as f32;
    let height = image.height as f32;
    let scale = (PREVIEW_WIDTH / width)
        .min(PREVIEW_HEIGHT / height)
        .min(1.0);
    let width = width * scale;
    let height = height * scale;
    bsn! {
        Name(name)
        template(move |_| Ok(ImageCard { session, call: call.clone(), extension, preview: Preview::Pending, export: Export::Idle }))
        Node { width: percent(100), flex_direction: FlexDirection::Column, row_gap: px(8), flex_shrink: 0.0 }
        Children [
            Text(metadata) WidgetryText--
            PreviewNode Name("ToolImagePreview") ImageNode::default() Visibility::Hidden
            Node { width: px(width), max_width: percent(100), height: px(height), flex_shrink: 0.0 }--
            @{exportable.then(|| bsn! {
                @AgentelierButton { @children: {Some(Box::new(bsn_list! { Text("另存为") WidgetryText }) as Box<dyn SceneList>)} }
                SaveButton Name("SaveGeneratedImage") Node { align_self: AlignSelf::Start } on(choose)
            })}--
            Text("正在读取预览…") WidgetryText ImageStatus Name("ToolImageStatus")
        ]
    }
}

#[derive(Resource, Default)]
struct PreviewRequest(Option<(u64, Entity)>);

fn start_preview(
    host: Res<AgentHost>,
    mut cards: Query<(Entity, &mut ImageCard)>,
    mut request: ResMut<PreviewRequest>,
) {
    if request.0.is_some() {
        return;
    }
    for (entity, mut card) in &mut cards {
        if !matches!(card.preview, Preview::Pending) {
            continue;
        }
        card.preview = match host.request_image_attachment(
            card.session,
            card.call.clone(),
            ImageAttachmentAction::Preview,
        ) {
            Ok(id) => {
                request.0 = Some((id, entity));
                Preview::Loading
            }
            Err(failure) => Preview::Failed(failure.to_string()),
        };
        break;
    }
}

fn receive(
    mut events: MessageReader<ImageAttachmentEvent>,
    mut failed: MessageReader<HostFailed>,
    mut request: ResMut<PreviewRequest>,
    mut cards: Query<(&mut ImageCard, &Children)>,
    mut previews: Query<(&mut ImageNode, &mut Visibility), With<PreviewNode>>,
    mut assets: ResMut<Assets<Image>>,
) {
    for event in events.read() {
        if let Some((_, entity)) = request.0.filter(|(id, _)| *id == event.operation_id) {
            request.0 = None;
            let Ok((mut card, children)) = cards.get_mut(entity) else {
                continue;
            };
            card.preview = match &event.result {
                Ok(ImageAttachmentReply::Preview {
                    width,
                    height,
                    rgba,
                }) => {
                    if let Some(child) = children.iter().find(|child| previews.contains(*child))
                        && let Ok((mut image, mut visibility)) = previews.get_mut(child)
                    {
                        image.image = assets.add(Image::new(
                            Extent3d {
                                width: *width,
                                height: *height,
                                depth_or_array_layers: 1,
                            },
                            TextureDimension::D2,
                            rgba.clone(),
                            TextureFormat::Rgba8UnormSrgb,
                            RenderAssetUsages::RENDER_WORLD,
                        ));
                        *visibility = Visibility::Inherited;
                        Preview::Ready
                    } else {
                        Preview::Failed("预览组件已不可用".into())
                    }
                }
                Ok(ImageAttachmentReply::Exported(_)) => {
                    Preview::Failed("收到不匹配的图片预览回执".into())
                }
                Err(failure) => Preview::Failed(failure.clone()),
            };
        } else {
            for (mut card, _) in &mut cards {
                if !matches!(card.export, Export::Writing(id) if id == event.operation_id) {
                    continue;
                }
                card.export = Export::Finished(match &event.result {
                    Ok(ImageAttachmentReply::Exported(path)) => format!(
                        "已保存原图副本 {}",
                        bevy_widgetry_path_display::display_path(path)
                    ),
                    Ok(ImageAttachmentReply::Preview { .. }) => "收到不匹配的图片导出回执".into(),
                    Err(failure) => format!("保存失败：{failure}"),
                });
                break;
            }
        }
    }
    if let Some(failure) = failed.read().last() {
        request.0 = None;
        for (mut card, _) in &mut cards {
            if matches!(card.preview, Preview::Pending | Preview::Loading) {
                card.preview = Preview::Failed(failure.reason.clone());
            }
            if matches!(card.export, Export::Writing(_)) {
                card.export = Export::Finished(format!("保存结果未确认：{}", failure.reason));
            }
        }
    }
}

fn choose(
    event: On<Activate>,
    parents: Query<&ChildOf>,
    mut cards: Query<&mut ImageCard>,
    pickers: Query<(), With<SavePicker>>,
    window: Single<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) -> Result {
    let card_entity = parents.get(event.entity)?.parent();
    let mut card = cards.get_mut(card_entity)?;
    if !pickers.is_empty() || matches!(card.export, Export::Choosing(_) | Export::Writing(_)) {
        return Ok(());
    }
    let extension = card.extension;
    let mut window = WidgetryFileDialogWindow {
        parent: Some(*window),
        modality: WidgetryFileDialogModality::Modal,
        ..default()
    };
    window.native.title = "另存生成图片".into();
    let picker = commands
        .spawn_scene_with_error_handler(bsn! {
            @WidgetryFileDialog {
                @mode: WidgetryFileDialogMode::SaveFile,
                @window: {Some(window)},
                @filters: {vec![WidgetryFileDialogFilter {
                    id: WidgetryFileDialogFilterId(extension.into()),
                    label: extension.to_ascii_uppercase(),
                    suffixes: vec![extension.into()],
                    default_extension: Some(extension.into()),
                }]},
                @storage_scope: {Some("generated-image-export".into())},
            }
            template(move |_| Ok(SavePicker(card_entity))) Name("GeneratedImageSavePicker")
            on(selected)
        })
        .id();
    card.export = Export::Choosing(picker);
    Ok(())
}

fn selected(
    event: On<WidgetryFileDialogResultEvent>,
    host: Res<AgentHost>,
    pickers: Query<&SavePicker>,
    mut cards: Query<&mut ImageCard>,
    mut commands: Commands,
) -> Result {
    let picker = pickers.get(event.entity)?;
    commands.entity(event.entity).despawn();
    let Ok(mut card) = cards.get_mut(picker.0) else {
        return Ok(());
    };
    card.export = match &event.result {
        WidgetryFileDialogResult::SavePath(path) => {
            match host.request_image_attachment(
                card.session,
                card.call.clone(),
                ImageAttachmentAction::Export(path.clone()),
            ) {
                Ok(id) => Export::Writing(id),
                Err(failure) => Export::Finished(format!("保存失败：{failure}")),
            }
        }
        WidgetryFileDialogResult::Cancelled => Export::Finished("已取消另存为".into()),
        _ => Export::Finished("文件选择器未返回保存路径".into()),
    };
    Ok(())
}

fn check_picker(
    pickers: Query<(Entity, &SavePicker)>,
    mut cards: Query<&mut ImageCard>,
    mut commands: Commands,
) {
    for (entity, picker) in &pickers {
        if !cards.contains(picker.0) {
            commands.entity(entity).despawn();
        }
    }
    for mut card in &mut cards {
        if let Export::Choosing(entity) = card.export
            && !pickers.contains(entity)
        {
            card.export = Export::Finished("文件选择器已关闭".into());
        }
    }
}

fn synchronize(
    cards: Query<(&ImageCard, &Children), Changed<ImageCard>>,
    mut statuses: Query<&mut Text, With<ImageStatus>>,
    buttons: Query<Has<InteractionDisabled>, With<SaveButton>>,
    mut commands: Commands,
) {
    for (card, children) in &cards {
        let preview = match &card.preview {
            Preview::Pending | Preview::Loading => "正在读取预览…".into(),
            Preview::Ready => String::new(),
            Preview::Failed(failure) => format!("预览失败：{failure}"),
        };
        let export = match &card.export {
            Export::Idle | Export::Choosing(_) => "",
            Export::Writing(_) => "正在保存原图副本…",
            Export::Finished(message) => message,
        };
        let text = [preview.as_str(), export]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        let busy = matches!(card.export, Export::Choosing(_) | Export::Writing(_));
        for child in children.iter() {
            if let Ok(mut status) = statuses.get_mut(child) {
                status.set_if_neq(Text::new(&text));
            }
            if let Ok(disabled) = buttons.get(child) {
                if busy && !disabled {
                    commands.entity(child).insert(InteractionDisabled);
                }
                if !busy && disabled {
                    commands.entity(child).remove::<InteractionDisabled>();
                }
            }
        }
    }
}
