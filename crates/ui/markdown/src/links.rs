use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use bevy_widgetry::button::WidgetryButton;
use bevy_widgetry::job::{
    JobId, JobManager, JobOptions, JobOutcome, JobRead, JobReceiver, SendStatus,
};
use bevy_widgetry::text::WidgetryText;
use std::io;
use tracing::{error, info};
#[derive(Component, Clone)]
struct LinkTarget(String);
#[derive(Component, Clone, Default)]
struct LinkLabel;
struct Pending {
    id: JobId,
    receiver: JobReceiver<io::Result<()>>,
    root: Entity,
    target: String,
    result: Option<Result<(), String>>,
}
#[derive(Resource, Default)]
struct LinkJobs(Vec<Pending>);
pub(crate) fn install(app: &mut App) {
    app.init_resource::<LinkJobs>().add_systems(Update, poll);
}
pub(crate) fn scene(target: &str) -> Box<dyn Scene> {
    let allowed = url::Url::parse(target)
        .is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some());
    let label = if allowed {
        format!("打开链接：{target}")
    } else {
        format!("链接目标（不打开）：{target}")
    };
    if !allowed {
        return Box::new(
            bsn! { Text(label) WidgetryText Node { min_width: px(0), flex_shrink: 0.0 } },
        );
    }
    let target = target.to_owned();
    Box::new(bsn! {
        @WidgetryButton { @children: {Some(Box::new(bsn_list! { Text(label) WidgetryText LinkLabel }) as Box<dyn SceneList>)} }
        template(move |_| Ok(LinkTarget(target.clone()))) Name("MarkdownOpenLink")
        Node { max_width: percent(100), align_self: AlignSelf::Start, flex_shrink: 0.0 } on(activate)
    })
}
fn activate(event: On<Activate>, targets: Query<&LinkTarget>, mut commands: Commands) -> Result {
    let root = event.entity;
    let target = targets.get(root)?.0.clone();
    commands.queue(move |world: &mut World| -> Result {
        if world
            .resource::<LinkJobs>()
            .0
            .iter()
            .any(|job| job.root == root)
        {
            return Ok(());
        }
        let address = target.clone();
        let (id, receiver) = world
            .resource::<JobManager>()
            .spawn_thread(
                JobOptions {
                    label: Some("打开正文链接".into()),
                    ..default()
                },
                move |context| {
                    if context.is_cancelled() {
                        context.acknowledge_cancel();
                        return Ok(());
                    }
                    let result = open(&address);
                    if context.send_blocking(result)? == SendStatus::Canceled {
                        context.acknowledge_cancel();
                    }
                    Ok(())
                },
            )
            .map_err(|failure| failure.into_bevy_error())?;
        world.resource_mut::<LinkJobs>().0.push(Pending {
            id,
            receiver,
            root,
            target,
            result: None,
        });
        info!(?id, "用户请求打开正文链接");
        Ok(())
    });
    Ok(())
}
fn open(address: &str) -> io::Result<()> {
    let parsed = url::Url::parse(address).map_err(io::Error::other)?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err(io::Error::other("仅允许 HTTP/HTTPS 链接"));
    }
    use winsafe::co;
    let _com = winsafe::CoInitializeEx(co::COINIT::APARTMENTTHREADED | co::COINIT::DISABLE_OLE1DDE)
        .map_err(io::Error::other)?;
    winsafe::ShellExecuteEx(&winsafe::SHELLEXECUTEINFO {
        mask: co::SEE_MASK::NOASYNC | co::SEE_MASK::FLAG_NO_UI,
        verb: Some("open"),
        file: address,
        show: co::SW::SHOWNORMAL,
        ..default()
    })
    .map_err(io::Error::other)
}
fn poll(world: &mut World) -> Result {
    let jobs = std::mem::take(&mut world.resource_mut::<LinkJobs>().0);
    for mut job in jobs {
        let outcome = loop {
            match job
                .receiver
                .try_recv()
                .map_err(|failure| failure.into_bevy_error())?
            {
                JobRead::Item(reply) => {
                    job.result = Some(
                        reply
                            .as_ref()
                            .as_ref()
                            .map(|_| ())
                            .map_err(ToString::to_string),
                    )
                }
                JobRead::Empty => break None,
                JobRead::Finished(outcome) => break Some(outcome),
                JobRead::Closed => return Err(io::Error::other("正文链接任务缺少终态").into()),
            }
        };
        let Some(outcome) = outcome else {
            world.resource_mut::<LinkJobs>().0.push(job);
            continue;
        };
        world
            .resource::<JobManager>()
            .remove(job.id)
            .map_err(|failure| failure.into_bevy_error())?;
        let result = match outcome {
            JobOutcome::Completed => job
                .result
                .ok_or_else(|| io::Error::other("正文链接任务缺少结果"))?,
            JobOutcome::Failed(failure) => Err(failure.to_string()),
            JobOutcome::Canceled => Ok(()),
        };
        if let Err(reason) = result {
            error!(error=%reason, "打开正文链接失败");
            let mut descendants = vec![job.root];
            while let Some(entity) = descendants.pop() {
                if let Some(children) = world.get::<Children>(entity) {
                    descendants.extend(children.iter());
                }
                if world.get::<LinkLabel>(entity).is_some()
                    && let Some(mut text) = world.get_mut::<Text>(entity)
                {
                    text.0 = format!("打开失败：{reason} · {}", job.target);
                }
            }
        } else {
            info!(id = ?job.id, "正文链接打开任务已结束");
        }
    }
    Ok(())
}
