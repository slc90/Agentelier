use crate::conversation::ChatState;
use agentelier_agent_host::{
    AgentHost, ComposerAssetOwner, ComposerAssetReply, ComposerAssetRequest, HostFailed,
};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::{AudioAttachment, SessionId};
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings};
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures::check_ready};
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy::window::RequestRedraw;
use bevy_widgetry::text::WidgetryText;
use std::time::{Duration, Instant};
use std::{io::Cursor, sync::Arc};

mod waveform;

const READ_TIMEOUT: Duration = Duration::from_secs(15);
const SINK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Component, Clone)]
struct AudioClip {
    owner: ComposerAssetOwner,
    attachment: AudioAttachment,
}

type AudioRead = Task<Result<Arc<[u8]>, String>>;

impl AudioClip {
    fn belongs(&self, chat: &ChatState) -> bool {
        match self.owner {
            ComposerAssetOwner::Draft(owner) => chat.composer.owner() == owner,
            ComposerAssetOwner::Session(owner) => {
                chat.session.as_ref().is_some_and(|chat| chat.id == owner)
            }
        }
    }

    fn read(&self, host: &AgentHost) -> Result<AudioRead> {
        let reply = host.composer_asset(ComposerAssetRequest::Audio {
            owner: self.owner,
            audio: self.attachment.clone(),
        })?;
        Ok(AsyncComputeTaskPool::get().spawn(async move {
            match reply.await {
                Ok(ComposerAssetReply::Audio(bytes)) => Ok(bytes),
                Ok(_) => Err("收到不匹配的音频附件回执".into()),
                Err(failure) => Err(failure.to_string()),
            }
        }))
    }
}

#[derive(Component, Clone, Default, PartialEq, Reflect)]
#[reflect(Component)]
struct AudioPlaybackStatus {
    phase: PlaybackPhase,
    position_ms: u64,
}

#[derive(Clone, Default, PartialEq, Reflect)]
enum PlaybackPhase {
    #[default]
    Ready,
    Loading,
    WaitingForOutput,
    Playing,
    Paused,
    Ended,
    Failed(String),
}

#[derive(Component, Clone, Default)]
struct PlaybackButton;
#[derive(Component, Clone, Default)]
struct PlaybackLabel;
#[derive(Component, Clone, Default)]
struct PlaybackError;

#[derive(Resource, Default)]
struct Playback {
    requested: Option<Entity>,
    active: Option<Active>,
}

struct Active {
    card: Entity,
    source: Source,
}

enum Source {
    Reading { task: AudioRead, expires: Instant },
    Starting { player: Entity, expires: Instant },
    Live(Output),
    Ended,
}

struct Output {
    player: Entity,
    position: Duration,
    last_progress: Instant,
}

pub(super) fn install(app: &mut App) {
    waveform::install(app);
    app.init_resource::<Playback>()
        .register_type::<AudioPlaybackStatus>()
        .add_systems(
            PostUpdate,
            (drive, present).chain().before(UiSystems::Content),
        );
}

pub(super) fn controls(
    session: SessionId,
    audio: &super::audio_attachment::ChatAudio,
) -> impl Scene + use<> {
    controls_for(ComposerAssetOwner::Session(session), audio)
}

pub(super) fn controls_for(
    owner: ComposerAssetOwner,
    audio: &super::audio_attachment::ChatAudio,
) -> impl Scene + use<> {
    let clip = AudioClip {
        owner,
        attachment: audio.attachment(),
    };
    let duration = time_label(clip.attachment.duration_ms);
    bsn! {
        Name("ChatAudioPlayback") template(move |_| Ok(clip.clone())) AudioPlaybackStatus
        Node { width: percent(100), min_width: px(0), align_items: AlignItems::Center, column_gap: px(8), row_gap: px(4), flex_wrap: FlexWrap::Wrap }
        Children [
            @AgentelierButton { @children: bsn_list! { Name("ChatAudioPlaybackLabel") PlaybackLabel Text("播放") WidgetryText } }
            Name("ChatAudioTogglePlayback") PlaybackButton on(activate)--
            @waveform::view()--
            Name("ChatAudioPlaybackTime") Text(duration) WidgetryText--
            Name("ChatAudioPlaybackError") PlaybackError Text::default() WidgetryText
            TextLayout::linebreak(LineBreak::AnyCharacter)
            Node { display: Display::None, width: percent(100), min_width: px(0) }
        ]
    }
}

fn activate(
    event: On<Activate>,
    buttons: Query<&ChildOf, With<PlaybackButton>>,
    mut playback: ResMut<Playback>,
) {
    if let Ok(card) = buttons.get(event.entity) {
        playback.requested = Some(card.parent());
    }
}

fn drive(
    world: &mut World,
    mut failures: Local<MessageCursor<HostFailed>>,
    mut exits: Local<MessageCursor<AppExit>>,
) {
    let host_failure = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|event| event.reason.clone());
    let exiting = exits.read(world.resource::<Messages<AppExit>>()).count() > 0;
    let now = Instant::now();
    world.resource_scope(|world, mut playback: Mut<Playback>| {
        let mut active = playback.active.take();
        let requested = playback.requested.take();
        if let Some(old) = active.take_if(|active| exiting || !belongs(world, active.card)) {
            reset(world, old);
        }
        if let Some(card) = requested.filter(|&card| !exiting && belongs(world, card)) {
            let restart = match active.as_mut().filter(|active| active.card == card) {
                None
                | Some(Active {
                    source: Source::Ended,
                    ..
                }) => true,
                Some(Active {
                    source: Source::Live(output),
                    ..
                }) => match world.get::<AudioSink>(output.player) {
                    Some(sink) if !sink.empty() => {
                        if sink.is_paused() {
                            output.last_progress = now;
                            sink.play();
                        } else {
                            sink.pause();
                        }
                        false
                    }
                    _ => true,
                },
                Some(_) => false,
            };
            if restart {
                if let Some(old) = active.take() {
                    reset(world, old);
                }
                active = request(world, card, now);
            }
        }

        if let Some(Active {
            card,
            source: Source::Reading { task, expires },
        }) = active.as_mut()
        {
            let result = check_ready(task)
                .or_else(|| host_failure.map(Err))
                .or_else(|| (now >= *expires).then(|| Err("读取音频超时".into())));
            if let Some(result) = result {
                match result.and_then(|bytes| start(world, bytes)) {
                    Ok(player) => {
                        set_status(world, *card, PlaybackPhase::WaitingForOutput, 0);
                        active = Some(Active {
                            card: *card,
                            source: Source::Starting {
                                player,
                                expires: now + SINK_TIMEOUT,
                            },
                        });
                    }
                    Err(reason) => {
                        set_status(world, *card, PlaybackPhase::Failed(reason), 0);
                        active = None;
                    }
                }
            }
        }

        playback.active = active.and_then(|active| tick(world, active, now));
    });
}

fn belongs(world: &World, card: Entity) -> bool {
    world
        .get::<AudioClip>(card)
        .is_some_and(|clip| clip.belongs(world.resource::<ChatState>()))
}

fn request(world: &mut World, card: Entity, now: Instant) -> Option<Active> {
    let clip = world.get::<AudioClip>(card)?;
    match clip.read(world.resource::<AgentHost>()) {
        Ok(task) => {
            set_status(world, card, PlaybackPhase::Loading, 0);
            Some(Active {
                card,
                source: Source::Reading {
                    task,
                    expires: now + READ_TIMEOUT,
                },
            })
        }
        Err(reason) => {
            set_status(world, card, PlaybackPhase::Failed(reason.to_string()), 0);
            None
        }
    }
}

fn start(world: &mut World, bytes: std::sync::Arc<[u8]>) -> Result<Entity, String> {
    let asset = AudioSource { bytes };
    // Bevy's AudioSource decoder assumes valid input. Check it with the same decoder first.
    rodio::Decoder::builder()
        .with_byte_len(asset.bytes.len() as u64)
        .with_data(Cursor::new(asset.clone()))
        .build()
        .map_err(|error| format!("无法解码音频：{error}"))?;
    let handle = world.resource_mut::<Assets<AudioSource>>().add(asset);
    Ok(world
        .spawn((
            Name::new("ChatAudioPlayer"),
            AudioPlayer(handle),
            PlaybackSettings::ONCE.paused(),
        ))
        .id())
}

fn tick(world: &mut World, mut active: Active, now: Instant) -> Option<Active> {
    let card = active.card;
    match &mut active.source {
        Source::Ended => return Some(active),
        Source::Reading { .. } => {}
        Source::Starting { player, expires } => {
            if let Some(sink) = world.get::<AudioSink>(*player) {
                sink.play();
                set_status(world, card, PlaybackPhase::Playing, 0);
                active.source = Source::Live(Output {
                    player: *player,
                    position: Duration::ZERO,
                    last_progress: now,
                });
            } else if now >= *expires {
                release_player(world, *player);
                set_status(
                    world,
                    card,
                    PlaybackPhase::Failed("音频输出设备不可用".into()),
                    0,
                );
                return None;
            }
        }
        Source::Live(output) => {
            let player = output.player;
            let Some(sink) = world.get::<AudioSink>(player) else {
                release_player(world, player);
                set_status(
                    world,
                    card,
                    PlaybackPhase::Failed("音频播放已中断".into()),
                    0,
                );
                return None;
            };
            let duration = world.get::<AudioClip>(card)?.attachment.duration_ms;
            if sink.empty() {
                release_player(world, player);
                set_status(world, card, PlaybackPhase::Ended, duration);
                active.source = Source::Ended;
                return Some(active);
            }
            let position = sink.position();
            let position_ms = position.as_millis().min(u128::from(duration)) as u64;
            if sink.is_paused() {
                set_status(world, card, PlaybackPhase::Paused, position_ms);
                return Some(active);
            }
            if position != output.position {
                output.position = position;
                output.last_progress = now;
            } else if now.duration_since(output.last_progress) >= SINK_TIMEOUT {
                release_player(world, player);
                set_status(
                    world,
                    card,
                    PlaybackPhase::Failed("音频输出已停止响应".into()),
                    position_ms,
                );
                return None;
            }
            set_status(world, card, PlaybackPhase::Playing, position_ms);
        }
    }
    world.write_message(RequestRedraw);
    Some(active)
}

fn release_player(world: &mut World, player: Entity) {
    if let Some(sink) = world.get::<AudioSink>(player) {
        sink.stop();
    }
    if let Some(handle) = world.get::<AudioPlayer>(player).map(|player| player.0.id()) {
        world.resource_mut::<Assets<AudioSource>>().remove(handle);
    }
    world.despawn(player);
}

fn reset(world: &mut World, active: Active) {
    if let Source::Starting { player, .. } | Source::Live(Output { player, .. }) = active.source {
        release_player(world, player);
    }
    set_status(world, active.card, PlaybackPhase::Ready, 0);
}

fn set_status(world: &mut World, card: Entity, phase: PlaybackPhase, position_ms: u64) {
    if let Some(mut state) = world.get_mut::<AudioPlaybackStatus>(card) {
        state.set_if_neq(AudioPlaybackStatus { phase, position_ms });
    }
}

fn present(
    cards: Query<(&AudioPlaybackStatus, &Children), Changed<AudioPlaybackStatus>>,
    buttons: Query<Has<InteractionDisabled>, With<PlaybackButton>>,
    descendants: Query<&Children>,
    mut labels: Query<&mut Text, (With<PlaybackLabel>, Without<PlaybackError>)>,
    mut errors: Query<(&mut Text, &mut Node), With<PlaybackError>>,
    mut commands: Commands,
) {
    for (state, children) in &cards {
        let (label, busy, reason) = match &state.phase {
            PlaybackPhase::Loading => ("正在加载", true, ""),
            PlaybackPhase::WaitingForOutput => ("正在连接音频输出", true, ""),
            PlaybackPhase::Playing => ("暂停", false, ""),
            PlaybackPhase::Paused => ("继续播放", false, ""),
            PlaybackPhase::Ended => ("重新播放", false, ""),
            PlaybackPhase::Failed(reason) => ("重试播放", false, reason.as_str()),
            PlaybackPhase::Ready => ("播放", false, ""),
        };
        for item in children.iter() {
            if let Ok((mut text, node)) = errors.get_mut(item) {
                text.set_if_neq(Text::new(reason));
                node.map_unchanged(|node| &mut node.display)
                    .set_if_neq(if reason.is_empty() {
                        Display::None
                    } else {
                        Display::Flex
                    });
            } else if let Ok(disabled) = buttons.get(item) {
                for child in descendants.iter_descendants(item) {
                    if let Ok(mut text) = labels.get_mut(child) {
                        text.set_if_neq(Text::new(label));
                        break;
                    }
                }
                if busy && !disabled {
                    commands.entity(item).insert(InteractionDisabled);
                }
                if !busy && disabled {
                    commands.entity(item).remove::<InteractionDisabled>();
                }
            }
        }
    }
}

fn time_label(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!(
        "{:02}:{:02}.{}",
        seconds / 60,
        seconds % 60,
        milliseconds % 1000 / 100
    )
}
