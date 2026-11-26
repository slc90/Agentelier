use crate::conversation::ChatState;
use agentelier_agent_host::{AgentHost, HostFailed, SavedAudioEvent};
use agentelier_controls::AgentelierButton;
use agentelier_conversation_bridge::SessionId;
use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings};
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::text::LineBreak;
use bevy::ui::{InteractionDisabled, UiSystems};
use bevy::ui_widgets::Activate;
use bevy::window::RequestRedraw;
use bevy_widgetry::text::WidgetryText;
use std::io::Cursor;
use std::time::{Duration, Instant};

mod waveform;

const READ_TIMEOUT: Duration = Duration::from_secs(15);
const SINK_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Component, Clone)]
struct AudioClip {
    session: SessionId,
    id: SessionId,
    duration_ms: u64,
    digest: String,
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
    Reading { operation: u64, expires: Instant },
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
    let clip = AudioClip {
        session,
        id: audio.id,
        duration_ms: audio.duration_ms,
        digest: audio.digest.clone(),
    };
    let duration = time_label(clip.duration_ms);
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
    mut receipts: Local<MessageCursor<SavedAudioEvent>>,
    mut failures: Local<MessageCursor<HostFailed>>,
    mut exits: Local<MessageCursor<AppExit>>,
) {
    let host_failure = failures
        .read(world.resource::<Messages<HostFailed>>())
        .last()
        .map(|event| event.reason.clone());
    let exiting = exits.read(world.resource::<Messages<AppExit>>()).count() > 0;
    let session = world
        .resource::<ChatState>()
        .session
        .as_ref()
        .map(|view| view.id);
    let now = Instant::now();
    world.resource_scope(|world, mut playback: Mut<Playback>| {
        let mut active = playback.active.take();
        let requested = playback.requested.take();
        if let Some(old) = active.take_if(|active| exiting || !belongs(world, active.card, session))
        {
            reset(world, old);
        }
        if let Some(card) = requested.filter(|&card| !exiting && belongs(world, card, session)) {
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

        let received = receipts.read(world.resource::<Messages<SavedAudioEvent>>());
        if let Some(Active {
            card,
            source: Source::Reading { operation, expires },
        }) = active.as_mut()
        {
            let result = received
                .filter(|reply| reply.operation_id == *operation)
                .last()
                .map(|reply| reply.result.clone())
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
        } else {
            received.count();
        }

        playback.active = active.and_then(|active| tick(world, active, now));
    });
}

fn belongs(world: &World, card: Entity, session: Option<SessionId>) -> bool {
    world
        .get::<AudioClip>(card)
        .is_some_and(|clip| Some(clip.session) == session)
}

fn request(world: &mut World, card: Entity, now: Instant) -> Option<Active> {
    let clip = world.get::<AudioClip>(card)?;
    match world
        .resource::<AgentHost>()
        .request_saved_audio(clip.session, clip.id)
    {
        Ok(operation) => {
            set_status(world, card, PlaybackPhase::Loading, 0);
            Some(Active {
                card,
                source: Source::Reading {
                    operation,
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
            let duration = world.get::<AudioClip>(card)?.duration_ms;
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
