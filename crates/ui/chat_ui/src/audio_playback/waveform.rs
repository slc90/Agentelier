use super::{AudioClip, AudioPlaybackStatus, AudioReadPurpose, READ_TIMEOUT};
use crate::conversation::ChatState;
use agentelier_agent_host::AgentHost;
use agentelier_session_store::{AudioExtrema, summarize_audio};
use bevy::asset::RenderAssetUsages;
use bevy::color::Alpha;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::ui::{CalculatedClip, ComputedUiRenderTargetInfo, UiSystems, widget::NodeImageMode};
use bevy::window::RequestRedraw;
use bevy_widgetry::{text::WidgetryText, theme::WidgetryTheme};
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

const CACHE_CAPACITY: usize = 32;
const MASK_HEIGHT: usize = 49;
const COLUMN_WIDTH: usize = 3;
const DISPLAY_COLUMNS: [usize; 4] = [32, 64, 128, 256];

#[derive(Component, Clone, Default)]
enum WaveformView {
    #[default]
    Waiting,
    Ready(Arc<[Mask]>),
    Unavailable,
}

#[derive(Component, Clone, Copy, Default)]
enum WaveformLayer {
    #[default]
    Unplayed,
    Played,
}

#[derive(Component, Clone, Default)]
struct WaveformHint;

struct Mask {
    width: f32,
    image: Handle<Image>,
}

#[derive(Resource, Default)]
pub(super) struct Waveforms {
    cache: VecDeque<(String, Arc<[Mask]>)>,
    pending: Option<Pending>,
}

impl Waveforms {
    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }
}

struct Pending {
    view: Entity,
    clip: AudioClip,
    job: Job,
}

enum Job {
    Reading {
        task: super::AudioRead,
        expires: Instant,
    },
    Building(Task<std::io::Result<Vec<Image>>>),
}

pub(super) fn install(app: &mut App) {
    app.init_resource::<Waveforms>().add_systems(
        PostUpdate,
        (load, present, super::update_clock)
            .chain()
            .after(UiSystems::PostLayout),
    );
}

pub(super) fn view() -> impl Scene {
    bsn! {
        Name("ChatAudioWaveform") WaveformView Pickable::IGNORE
        Node { height: px(28), flex_basis: px(72), flex_grow: 1.0, max_width: px(400), align_items: AlignItems::Center, justify_content: JustifyContent::Center, overflow: Overflow::clip() }
        Children [
            Name("ChatAudioWaveformUnplayed") WaveformLayer::Unplayed Pickable::IGNORE
            ImageNode { image_mode: NodeImageMode::Stretch, color: Color::NONE }
            Node { position_type: PositionType::Absolute, width: percent(100), height: percent(100), left: px(0) }--
            Name("ChatAudioWaveformPlayed") WaveformLayer::Played Pickable::IGNORE
            ImageNode { image_mode: NodeImageMode::Stretch, color: Color::NONE }
            Node { position_type: PositionType::Absolute, width: percent(0), height: percent(100), left: px(0) }--
            Name("ChatAudioWaveformHint") WaveformHint Pickable::IGNORE Text("正在计算波形…") WidgetryText
        ]
    }
}

fn load(
    mut waveforms: ResMut<Waveforms>,
    mut images: ResMut<Assets<Image>>,
    host: Res<AgentHost>,
    chat: Res<ChatState>,
    clips: Query<&AudioClip>,
    mut views: Query<(
        Entity,
        &ChildOf,
        &mut WaveformView,
        &ComputedNode,
        &UiGlobalTransform,
        &ComputedUiRenderTargetInfo,
        Option<&CalculatedClip>,
        &InheritedVisibility,
    )>,
) {
    if let Some(mut pending) = waveforms.pending.take() {
        let result = poll(&mut pending);
        match result {
            Some(Ok(masks)) => {
                let masks = masks
                    .into_iter()
                    .map(|image| Mask {
                        width: image.width() as f32,
                        image: images.add(image),
                    })
                    .collect::<Arc<[_]>>();
                if waveforms.cache.len() == CACHE_CAPACITY {
                    waveforms.cache.pop_front();
                }
                waveforms
                    .cache
                    .push_back((pending.clip.attachment.digest, masks));
            }
            Some(Err(error)) => {
                tracing::error!(audio = %pending.clip.attachment.id, %error, "生成音频波形失败");
                if let Ok((_, _, mut state, ..)) = views.get_mut(pending.view) {
                    *state = WaveformView::Unavailable;
                }
            }
            None => waveforms.pending = Some(pending),
        }
    }

    for (entity, parent, mut state, node, transform, target, clip, visible) in &mut views {
        if !matches!(*state, WaveformView::Waiting)
            || !visible.get()
            || !on_screen(node, transform, target, clip)
        {
            continue;
        }
        let Ok(clip) = clips.get(parent.parent()) else {
            continue;
        };
        if !clip.belongs(&chat) {
            continue;
        }
        if let Some(index) = waveforms
            .cache
            .iter()
            .position(|(hash, _)| *hash == clip.attachment.digest)
            && let Some(entry) = waveforms.cache.remove(index)
        {
            *state = WaveformView::Ready(entry.1.clone());
            waveforms.cache.push_back(entry);
        } else if waveforms.pending.is_none() {
            match clip.read(&host, AudioReadPurpose::Waveform) {
                Ok(task) => {
                    waveforms.pending = Some(Pending {
                        view: entity,
                        clip: clip.clone(),
                        job: Job::Reading {
                            task,
                            expires: Instant::now() + READ_TIMEOUT,
                        },
                    });
                }
                Err(error) => {
                    tracing::error!(audio = %clip.attachment.id, %error, "请求音频波形失败");
                    *state = WaveformView::Unavailable;
                }
            }
        }
    }
}

fn poll(pending: &mut Pending) -> Option<Result<Vec<Image>, String>> {
    match &mut pending.job {
        Job::Reading { task, expires } => match check_ready(task) {
            Some(Ok(bytes)) => {
                let duration_ms = pending.clip.attachment.duration_ms;
                pending.job = Job::Building(AsyncComputeTaskPool::get().spawn(async move {
                    summarize_audio(bytes, duration_ms).map(|peaks| {
                        DISPLAY_COLUMNS
                            .into_iter()
                            .map(|columns| mask(&peaks, columns))
                            .collect()
                    })
                }));
                None
            }
            Some(Err(error)) => Some(Err(error)),
            None if Instant::now() >= *expires => Some(Err("读取音频超时".into())),
            None => None,
        },
        Job::Building(task) => {
            check_ready(task).map(|result| result.map_err(|error| error.to_string()))
        }
    }
}

fn on_screen(
    node: &ComputedNode,
    transform: &UiGlobalTransform,
    target: &ComputedUiRenderTargetInfo,
    clip: Option<&CalculatedClip>,
) -> bool {
    let size = node.size();
    if size.x <= 0.0 || size.y <= 0.0 {
        return false;
    }
    let center = transform.translation;
    let rect = Rect::from_center_size(center, size);
    if rect
        .intersect(Rect::from_corners(
            Vec2::ZERO,
            target.physical_size().as_vec2(),
        ))
        .is_empty()
    {
        return false;
    }
    clip.is_none_or(|clip| {
        clip.rects().is_some_and(|clips| {
            clips.iter().all(|clip| {
                let local = Rect::from_corners(
                    clip.world_to_clip_local.transform_point2(rect.min),
                    clip.world_to_clip_local.transform_point2(rect.max),
                );
                !clip.rect.intersect(local).is_empty()
            })
        })
    })
}

fn mask(peaks: &[AudioExtrema], max_columns: usize) -> Image {
    let columns = peaks.len().min(max_columns);
    let width = columns * COLUMN_WIDTH;
    let mut pixels = vec![255_u8; width * MASK_HEIGHT * 4];
    let middle = (MASK_HEIGHT / 2) as f32;
    for col in 0..columns {
        let start = col * peaks.len() / columns;
        let end = (col + 1) * peaks.len() / columns;
        let range = peaks[start..end]
            .iter()
            .fold(AudioExtrema::default(), |range, peak| AudioExtrema {
                min: range.min.min(peak.min),
                max: range.max.max(peak.max),
            });
        let high = (middle * (1.0 - range.max)).round() as usize;
        let low = (middle * (1.0 - range.min)).round() as usize;
        for row in 0..MASK_HEIGHT {
            for dx in 0..COLUMN_WIDTH {
                let alpha = (row * width + col * COLUMN_WIDTH + dx) * 4 + 3;
                if !(high..=low).contains(&row) || dx == COLUMN_WIDTH - 1 {
                    pixels[alpha] = 0;
                }
            }
        }
    }
    let mut image = Image::new(
        Extent3d {
            width: width as u32,
            height: MASK_HEIGHT as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        pixels,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::nearest();
    image
}

fn present(
    theme: Res<WidgetryTheme>,
    views: Query<(&WaveformView, &ChildOf, &ComputedNode, &Children)>,
    playback: Query<(&AudioClip, &AudioPlaybackStatus)>,
    mut layers: Query<(&WaveformLayer, &mut ImageNode, &mut Node), Without<WaveformHint>>,
    mut hints: Query<(&mut Text, &mut Node), With<WaveformHint>>,
    mut redraw: MessageWriter<RequestRedraw>,
) {
    for (state, parent, layout, children) in &views {
        let Ok((clip, playback)) = playback.get(parent.parent()) else {
            continue;
        };
        let mask = match state {
            WaveformView::Ready(masks) => masks
                .iter()
                .rfind(|mask| mask.width <= layout.size().x)
                .or_else(|| masks.first()),
            _ => None,
        };
        let progress = playback.position_ms as f32 / clip.attachment.duration_ms.max(1) as f32;
        let accent = theme.slider.normal.fill;
        for child in children.iter() {
            if let Ok((mut text, node)) = hints.get_mut(child) {
                let hint = match state {
                    WaveformView::Waiting => "正在计算波形…",
                    WaveformView::Unavailable => "波形不可用",
                    WaveformView::Ready(_) => "",
                };
                let display = if hint.is_empty() {
                    Display::None
                } else {
                    Display::Flex
                };
                if text.set_if_neq(Text::new(hint))
                    | node
                        .map_unchanged(|node| &mut node.display)
                        .set_if_neq(display)
                {
                    redraw.write(RequestRedraw);
                }
            } else if let Some(mask) = mask
                && let Ok((layer, mut image, node)) = layers.get_mut(child)
            {
                let changed = image
                    .reborrow()
                    .map_unchanged(|image| &mut image.image)
                    .set_if_neq(mask.image.clone());
                let changed = match layer {
                    WaveformLayer::Unplayed => {
                        image
                            .map_unchanged(|image| &mut image.color)
                            .set_if_neq(accent.with_alpha(0.4))
                            | changed
                    }
                    WaveformLayer::Played => {
                        let extent = Vec2::new(mask.width * progress, MASK_HEIGHT as f32);
                        image
                            .reborrow()
                            .map_unchanged(|image| &mut image.color)
                            .set_if_neq(accent)
                            | image
                                .map_unchanged(|image| &mut image.rect)
                                .set_if_neq(Some(Rect::from_corners(Vec2::ZERO, extent)))
                            | node
                                .map_unchanged(|node| &mut node.width)
                                .set_if_neq(percent(100.0 * progress))
                            | changed
                    }
                };
                if changed {
                    redraw.write(RequestRedraw);
                }
            }
        }
    }
}
