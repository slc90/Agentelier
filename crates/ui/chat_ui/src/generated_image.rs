use crate::conversation::ChatState;
use agentelier_agent_host::{
    AgentHost, ImageAttachmentAction, ImageAttachmentEvent, ImageAttachmentReply,
};
use agentelier_conversation_bridge::{SessionId, ToolView};
use bevy::asset::RenderAssetUsages;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::text::{Justify, LineBreak};
use bevy_widgetry::text::WidgetryText;
use std::io;

const MAX_FRAME_WIDTH: f32 = 512.0;
const MAX_FRAME_HEIGHT: f32 = 360.0;
const CHANNELS: u32 = 4;

#[derive(Clone, PartialEq, Reflect)]
pub(super) enum Artifact {
    Generating,
    Failed(String),
    Cancelled,
    Saved {
        digest: String,
        width: u32,
        height: u32,
        mime: String,
    },
}

impl Artifact {
    pub(super) fn project(tool: &ToolView) -> Option<Self> {
        if !tool.image_generation || !tool.retained || tool.collaboration {
            return None;
        }
        if !tool.result_saved {
            return matches!(tool.status.as_str(), "receiving" | "pending" | "running")
                .then_some(Self::Generating);
        }
        if let Some(image) = tool.image.as_ref().filter(|image| image.exportable) {
            return Some(Self::Saved {
                digest: image.digest.clone(),
                width: image.width,
                height: image.height,
                mime: image.mime.clone(),
            });
        }
        if tool.status == "cancelled" {
            return Some(Self::Cancelled);
        }
        Some(Self::Failed(failure_reason(tool.output.as_deref())))
    }
}

fn failure_reason(output: Option<&str>) -> String {
    let fallback = "没有收到已确认的图片";
    let Some(output) = output.map(str::trim).filter(|text| !text.is_empty()) else {
        return fallback.into();
    };
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        return value
            .pointer("/error/message")
            .or_else(|| value.get("message"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(fallback)
            .to_owned();
    }
    output.to_owned()
}

#[derive(Component, Clone)]
struct Preview {
    session: SessionId,
    call_id: String,
    artifact: Option<Artifact>,
    load: PreviewLoad,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PreviewLoad {
    Idle,
    Queued,
    Loading(u64),
}

#[derive(Component, Clone, Default)]
struct Frame;

#[derive(Component, Clone, Default)]
struct Caption;

#[derive(Component, Clone, Default)]
struct Metadata;

pub(super) fn card(session: SessionId, call_id: String) -> impl Scene {
    let state = Preview {
        session,
        call_id,
        artifact: None,
        load: PreviewLoad::Idle,
    };
    bsn! {
        Name("ChatGeneratedImage") template(move |_| Ok(state.clone()))
        Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(4) }
        Children [
            Frame Name("ChatGeneratedImageFrame") ImageNode::default()
            Node { align_self: AlignSelf::Start, min_width: px(0), max_width: percent(100), align_items: AlignItems::Center }
            Children [
                Caption Text::default() WidgetryText
                TextLayout { justify: Justify::Center, linebreak: LineBreak::AnyCharacter }
                Node { width: percent(100), min_width: px(0) }
            ]--
            Metadata Text::default() WidgetryText Node { display: Display::None }
        ]
    }
}

pub(super) fn reconcile(world: &mut World, root: Entity, artifact: Artifact) -> Result {
    let state = world
        .get::<Preview>(root)
        .ok_or_else(|| io::Error::other("图片缺少预览状态"))?;
    if state.artifact.as_ref() == Some(&artifact) {
        return Ok(());
    }
    let frame = child::<Frame>(world, root)?;
    let meta = child::<Metadata>(world, root)?;
    let (width, ratio, metadata, load, caption) = match &artifact {
        Artifact::Generating => (
            percent(100),
            None,
            String::new(),
            PreviewLoad::Idle,
            "正在生成图片…".into(),
        ),
        Artifact::Failed(reason) => (
            percent(100),
            None,
            String::new(),
            PreviewLoad::Idle,
            format!("图片生成失败：{reason}"),
        ),
        Artifact::Cancelled => (
            percent(100),
            None,
            String::new(),
            PreviewLoad::Idle,
            "图片生成已取消".into(),
        ),
        Artifact::Saved {
            width,
            height,
            mime,
            ..
        } => {
            let ratio = *width as f32 / *height as f32;
            let preferred = MAX_FRAME_WIDTH.min(MAX_FRAME_HEIGHT * ratio);
            let format = if mime == "image/jpeg" { "JPEG" } else { "PNG" };
            let metadata = format!("{format} · {width} × {height}");
            (
                px(preferred),
                Some(ratio),
                metadata,
                PreviewLoad::Queued,
                "正在读取图片预览…".into(),
            )
        }
    };
    let mut layout = world
        .get_mut::<Node>(frame)
        .ok_or_else(|| io::Error::other("图片预览缺少布局"))?;
    layout.width = width;
    layout.aspect_ratio = ratio;
    let mut node = world
        .get_mut::<Node>(meta)
        .ok_or_else(|| io::Error::other("图片元数据缺少布局"))?;
    node.display = if metadata.is_empty() {
        Display::None
    } else {
        Display::Flex
    };
    set_text(world, meta, metadata)?;
    world.entity_mut(frame).insert(ImageNode::default());
    let caption_entity = child::<Caption>(world, frame)?;
    set_text(world, caption_entity, caption)?;
    let mut state = world
        .get_mut::<Preview>(root)
        .ok_or_else(|| io::Error::other("图片缺少预览状态"))?;
    state.artifact = Some(artifact);
    state.load = load;
    Ok(())
}

pub(super) fn receive(
    world: &mut World,
    mut cursor: Local<MessageCursor<ImageAttachmentEvent>>,
) -> Result {
    let session = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|session| session.id);
    let mut previews = world.query::<(Entity, &Preview)>();
    let pending = previews
        .iter(world)
        .filter(|(_, preview)| Some(preview.session) == session)
        .find_map(|(entity, preview)| match preview.load {
            PreviewLoad::Loading(operation) => Some((operation, entity)),
            _ => None,
        });
    let update = cursor
        .read(world.resource::<Messages<ImageAttachmentEvent>>())
        .filter(|event| pending.is_some_and(|(operation, _)| operation == event.operation_id))
        .last()
        .map(|event| match &event.result {
            Ok(ImageAttachmentReply::Preview {
                width,
                height,
                rgba,
            }) => preview_image(*width, *height, rgba.clone()),
            Ok(ImageAttachmentReply::Exported(_)) => Err("图片预览回执类型不匹配".into()),
            Err(failure) => Err(failure.clone()),
        });
    if let Some((_, root)) = pending {
        let Some(result) = update else {
            return Ok(());
        };
        let frame = child::<Frame>(world, root)?;
        let caption = child::<Caption>(world, frame)?;
        match result {
            Ok(image) => {
                let handle = world.resource_mut::<Assets<Image>>().add(image);
                world.entity_mut(frame).insert(ImageNode::new(handle));
                set_text(world, caption, String::new())?;
            }
            Err(failure) => set_text(world, caption, format!("图片预览不可用：{failure}"))?,
        }
        let mut preview = world
            .get_mut::<Preview>(root)
            .ok_or_else(|| io::Error::other("图片预览已不存在"))?;
        preview.load = PreviewLoad::Idle;
    }
    let next = previews
        .iter(world)
        .find(|(_, preview)| {
            Some(preview.session) == session && preview.load == PreviewLoad::Queued
        })
        .map(|(root, preview)| (root, preview.session, preview.call_id.clone()));
    if let Some((root, session, call_id)) = next {
        let request = world.resource::<AgentHost>().request_image_attachment(
            session,
            call_id,
            ImageAttachmentAction::Preview,
        );
        let load = match request {
            Ok(operation) => PreviewLoad::Loading(operation),
            Err(failure) => {
                let frame = child::<Frame>(world, root)?;
                let caption = child::<Caption>(world, frame)?;
                set_text(world, caption, format!("图片预览不可用：{failure}"))?;
                PreviewLoad::Idle
            }
        };
        world
            .get_mut::<Preview>(root)
            .ok_or_else(|| io::Error::other("图片预览已不存在"))?
            .load = load;
    }
    Ok(())
}

fn preview_image(width: u32, height: u32, rgba: Vec<u8>) -> std::result::Result<Image, String> {
    let expected = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(CHANNELS));
    if expected == Some(0) || expected.map(|size| size as usize) != Some(rgba.len()) {
        return Err("图片预览尺寸与像素长度不一致".into());
    }
    Ok(Image::new(
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

fn child<T: Component>(world: &World, parent: Entity) -> Result<Entity> {
    world
        .get::<Children>(parent)
        .and_then(|children| {
            children
                .iter()
                .find(|child| world.get::<T>(*child).is_some())
        })
        .ok_or_else(|| io::Error::other("图片预览缺少子控件").into())
}

fn set_text(world: &mut World, entity: Entity, value: String) -> Result {
    world
        .get_mut::<Text>(entity)
        .ok_or_else(|| io::Error::other("图片预览缺少文字"))?
        .set_if_neq(Text(value));
    Ok(())
}
