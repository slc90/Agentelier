mod atlas;
mod glyphs;
mod portrait;

use crate::diagnostics;
use agentelier_login_ui::{BeginDataDirectorySetup, LoginUiHost};
use bevy::camera::RenderTarget;
use bevy::ecs::message::MessageCursor;
use bevy::prelude::*;
use bevy::render::view::Msaa;
use bevy::ui::widget::NodeImageMode;
use bevy::window::{PrimaryWindow, RequestRedraw, WindowRef, WindowResolution};
use bevy::winit::{WINIT_WINDOWS, WinitSettings};
use bevy_app_assets::MainWindowBackground;
use bevy_widgetry::app_logging::safe_diagnostic;
use bevy_widgetry::scene::WidgetrySceneCommandsExt;
use bevy_widgetry::splash::{Splash, SplashCompleted, SplashPlugin, SplashShown, SplashSystems};
use bevy_widgetry::ui_scale::WidgetryUiScaleSystems;
use std::{io, time::Duration};
use tracing::{debug, error};

const PLAYBACK_DURATION: Duration = Duration::from_millis(4820);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Resource, Default)]
enum StartupSplash {
    #[default]
    Pending,
    AwaitingWindow(Duration),
    Active(ActiveSplash),
    Done,
}

struct ActiveSplash {
    main: Entity,
    window: Entity,
    main_ready: bool,
    scene: Option<SplashScene>,
    phase: Phase,
}

struct SplashScene {
    camera: Entity,
    image: Entity,
    glyphs: glyphs::GlyphScene,
}

enum Phase {
    Loading(Duration),
    AwaitingFirstFrame(Duration),
    Playing(Duration),
    HandingOff(Duration),
}

impl Phase {
    fn stalled(&self, now: Duration) -> Option<&'static str> {
        let (since, stage) = match *self {
            Self::Loading(since) => (since, "加载画面"),
            Self::AwaitingFirstFrame(since) => (since, "确认 Splash 首帧"),
            Self::HandingOff(since) => (since, "确认主窗口交接帧"),
            Self::Playing(_) => return None,
        };
        (now.saturating_sub(since) >= STARTUP_TIMEOUT).then_some(stage)
    }
}

pub(crate) fn install(app: &mut App) {
    glyphs::install(app);
    app.add_plugins(SplashPlugin)
        .init_resource::<StartupSplash>()
        .add_observer(shown)
        .add_observer(completed)
        .add_systems(
            PostUpdate,
            drive
                .after(WidgetryUiScaleSystems::Sync)
                .before(SplashSystems::Geometry)
                .run_if(|startup: Res<StartupSplash>| !matches!(*startup, StartupSplash::Done)),
        );
}

fn drive(world: &mut World) -> Result {
    let now = world.resource::<Time<Real>>().elapsed();
    world.resource_scope(
        |world, mut startup: Mut<StartupSplash>| match &mut *startup {
            StartupSplash::Done => Ok(()),
            StartupSplash::Pending => {
                *startup = StartupSplash::AwaitingWindow(now);
                Ok(())
            }
            StartupSplash::AwaitingWindow(since) => {
                let main = world
                    .query_filtered::<Entity, With<PrimaryWindow>>()
                    .single(world)
                    .map_err(diagnostics::operation)?;
                let Some(size) = native_size(world, main) else {
                    if now.saturating_sub(*since) < STARTUP_TIMEOUT {
                        return Ok(());
                    }
                    *startup = StartupSplash::Done;
                    return fallback(world, main, None, &timeout("准备主窗口"));
                };
                match start(world, main, size, now) {
                    Ok(active) => *startup = StartupSplash::Active(active),
                    Err(failure) => {
                        *startup = StartupSplash::Done;
                        fallback(world, main, None, &failure)?;
                    }
                }
                Ok(())
            }
            StartupSplash::Active(active) => {
                if let Err(failure) = advance(world, active, now)
                    && let StartupSplash::Active(active) =
                        std::mem::replace(&mut *startup, StartupSplash::Done)
                {
                    fallback(world, active.main, Some(active), &failure)?;
                }
                Ok(())
            }
        },
    )
}

fn native_size(world: &World, main: Entity) -> Option<UVec2> {
    let expected = world.get::<Window>(main)?.physical_size();
    WINIT_WINDOWS.with_borrow(|windows| {
        let size = windows.get_window(main)?.inner_size();
        (expected == UVec2::new(size.width, size.height)).then_some(expected)
    })
}

fn start(world: &mut World, main: Entity, size: UVec2, now: Duration) -> Result<ActiveSplash> {
    let window = Splash::start(
        world,
        main,
        Window {
            title: "Agentelier Splash".into(),
            resolution: WindowResolution::new(size.x, size.y),
            focused: false,
            skip_taskbar: true,
            ..default()
        },
    )
    .map_err(|failure| failure.into_bevy_error())?;
    debug!(?size, "已创建 Splash 窗口");
    Ok(ActiveSplash {
        main,
        window,
        main_ready: false,
        scene: None,
        phase: Phase::Loading(now),
    })
}

fn advance(world: &mut World, active: &mut ActiveSplash, now: Duration) -> Result {
    if let Some(stage) = active.phase.stalled(now) {
        return Err(timeout(stage));
    }
    if Splash::phase(world, active.window).is_none() {
        return Err(diagnostics::operation(io::Error::other(
            "Splash 窗口会话在交接之前结束",
        )));
    }
    if !active.main_ready
        && world
            .query_filtered::<Entity, With<LoginUiHost>>()
            .iter(world)
            .next()
            .is_some()
    {
        Splash::main_ready(world, active.window).map_err(|failure| failure.into_bevy_error())?;
        active.main_ready = true;
        debug!("主窗口基础界面已挂载");
    }
    match active.phase {
        Phase::Loading(_) => {
            if active.scene.is_none() {
                let image = world.resource::<MainWindowBackground>().0.clone();
                if let Some(bevy::asset::LoadState::Failed(failure)) =
                    world.resource::<AssetServer>().get_load_state(image.id())
                {
                    return Err(diagnostics::operation(failure));
                }
                let Some(image_size) = world
                    .resource::<Assets<Image>>()
                    .get(&image)
                    .map(Image::size)
                else {
                    return Ok(());
                };
                let size = world
                    .get::<Window>(active.window)
                    .ok_or_else(|| {
                        diagnostics::operation(io::Error::other("Splash 窗口在初始化期间消失"))
                    })?
                    .physical_size();
                active.scene = Some(render_scene(world, active.window, size, image, image_size)?);
            }
            let Some(scene) = active.scene.as_ref() else {
                return Ok(());
            };
            if scene.glyphs.ready().map_err(diagnostics::operation)? {
                Splash::ready(world, active.window).map_err(|failure| failure.into_bevy_error())?;
                active.phase = Phase::AwaitingFirstFrame(now);
            }
        }
        Phase::Playing(shown_at) if now.saturating_sub(shown_at) >= PLAYBACK_DURATION => {
            if let Some(scene) = &active.scene {
                world
                    .entity_mut(scene.glyphs.batch)
                    .insert(Visibility::Hidden);
                world.entity_mut(scene.image).insert(Visibility::Visible);
            }
            Splash::finished(world, active.window).map_err(|failure| failure.into_bevy_error())?;
            active.phase = Phase::HandingOff(now);
            debug!("Splash 播放结束，等待主窗口呈现");
        }
        _ => {}
    }
    Ok(())
}

fn render_scene(
    world: &mut World,
    window: Entity,
    size: UVec2,
    artwork: Handle<Image>,
    image_size: UVec2,
) -> Result<SplashScene> {
    let image_size = image_size.as_vec2();
    let size = size.as_vec2();
    let scale = (size / image_size).max_element();
    let crop = (size / scale).min(image_size);
    let min = (image_size - crop) * 0.5;
    let crop = Rect::from_corners(min, min + crop);
    let glyphs = glyphs::spawn(world, artwork.clone(), size.as_uvec2(), crop)?;
    let camera = world
        .spawn((
            Name::new("SplashCamera"),
            Camera2d,
            Camera {
                clear_color: ClearColorConfig::Custom(Color::srgb_u8(3, 19, 12)),
                ..default()
            },
            RenderTarget::Window(WindowRef::Entity(window)),
            Msaa::Off,
            glyphs::layer(),
        ))
        .id();
    let rect = Some(crop);
    let image = world
        .commands()
        .spawn_scene_with_error_handler(bsn! {
            Name("SplashArtwork") UiTargetCamera(camera) Pickable::IGNORE Visibility::Hidden
            ImageNode { image: artwork, rect: rect, image_mode: NodeImageMode::Stretch }
            Node { width: percent(100), height: percent(100) }
        })
        .id();
    Ok(SplashScene {
        camera,
        image,
        glyphs,
    })
}

fn shown(event: On<SplashShown>, mut startup: ResMut<StartupSplash>, now: Res<Time<Real>>) {
    if let StartupSplash::Active(active) = &mut *startup
        && active.window == event.splash
        && matches!(active.phase, Phase::AwaitingFirstFrame(_))
    {
        active.phase = Phase::Playing(now.elapsed());
        debug!("Splash 首帧已呈现，开始播放");
    }
}

fn completed(event: On<SplashCompleted>, mut commands: Commands) {
    let splash = event.splash;
    commands.queue(move |world: &mut World| -> Result {
        let mut startup = world.resource_mut::<StartupSplash>();
        if !matches!(&*startup, StartupSplash::Active(active) if active.window == splash) {
            return Ok(());
        }
        if let StartupSplash::Active(active) = std::mem::replace(&mut *startup, StartupSplash::Done)
        {
            let main = active.main;
            cleanup(world, active)?;
            debug!("Splash 已完成主窗口交接");
            begin_directory_setup(world, main);
        }
        Ok(())
    });
}

fn fallback(
    world: &mut World,
    main: Entity,
    active: Option<ActiveSplash>,
    failure: &BevyError,
) -> Result {
    let cleanup_result = active.map_or(Ok(()), |active| cleanup(world, active));
    let mut window = world
        .get_mut::<Window>(main)
        .ok_or_else(|| diagnostics::operation(io::Error::other("Splash 回退缺少主窗口")))?;
    window.visible = true;
    error!(
        error = %safe_diagnostic(format_args!("{failure:?}")),
        "Splash 初始化或交接失败，显示主窗口并继续启动"
    );
    begin_directory_setup(world, main);
    cleanup_result
}

fn begin_directory_setup(world: &mut World, main: Entity) {
    world.insert_resource(WinitSettings::desktop_app());
    world.write_message(RequestRedraw);
    world.write_message(BeginDataDirectorySetup { main });
}

fn cleanup(world: &mut World, active: ActiveSplash) -> Result {
    if let Some(scene) = active.scene {
        scene.glyphs.retire(world);
        world.despawn(scene.image);
        world.despawn(scene.camera);
    }
    if Splash::phase(world, active.window).is_some() {
        Splash::cancel(world, active.window).map_err(|failure| failure.into_bevy_error())?;
    }
    Ok(())
}

pub(crate) fn retire(world: &mut World, mut exits: Local<MessageCursor<AppExit>>) -> Result {
    if exits.read(world.resource::<Messages<AppExit>>()).count() == 0 {
        return Ok(());
    }
    let mut startup = world.resource_mut::<StartupSplash>();
    if let StartupSplash::Active(active) = std::mem::replace(&mut *startup, StartupSplash::Done) {
        cleanup(world, active)?;
        debug!("退出时已清理 Splash 会话");
    }
    world.insert_resource(WinitSettings::desktop_app());
    Ok(())
}

fn timeout(stage: &str) -> BevyError {
    diagnostics::operation(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("Splash 等待超时：{stage}"),
    ))
}
