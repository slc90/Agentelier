use agentelier_agent_host::{GlobalTasksEvent, ReceiveAgentEvents};
use agentelier_conversation_bridge::{
    GlobalTaskProcess, GlobalTaskStatus, SessionId, TaskDelivery,
};
use bevy::prelude::*;

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
pub(crate) struct GlobalTaskState {
    pub(crate) revision: u64,
    pub(crate) groups: Vec<Group>,
}

#[derive(Reflect)]
pub(crate) struct Group {
    pub(crate) session: SessionId,
    pub(crate) title: String,
    pub(crate) tasks: Vec<Task>,
}

#[derive(Reflect)]
pub(crate) struct Task {
    pub(crate) id: SessionId,
    pub(crate) name: String,
    pub(crate) status: &'static str,
    delivery: &'static str,
    process: &'static str,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GlobalTaskState>()
        .register_type::<GlobalTaskState>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

fn receive(mut events: MessageReader<GlobalTasksEvent>, mut state: ResMut<GlobalTaskState>) {
    for GlobalTasksEvent(snapshot) in events.read() {
        if snapshot.revision <= state.revision {
            continue;
        }
        state.revision = snapshot.revision;
        state.groups = snapshot
            .groups
            .iter()
            .map(|group| Group {
                session: group.session,
                title: group.title.clone(),
                tasks: group
                    .tasks
                    .iter()
                    .map(|task| Task {
                        id: task.id.0,
                        name: task.name.clone(),
                        status: match task.status {
                            GlobalTaskStatus::Waiting => "等待执行",
                            GlobalTaskStatus::Running => "运行中",
                            GlobalTaskStatus::WaitingForResources => "等待资源",
                            GlobalTaskStatus::Cancelling => "取消中",
                            GlobalTaskStatus::Failed => "失败",
                            GlobalTaskStatus::Cancelled => "已取消",
                            GlobalTaskStatus::ResultReady => "已保存待处理",
                            GlobalTaskStatus::NeedsConfirmation => "等待确认",
                        },
                        delivery: match task.delivery {
                            TaskDelivery::AwaitingResult => "awaiting_result",
                            TaskDelivery::Saved => "saved",
                            TaskDelivery::Appended => "appended",
                            TaskDelivery::Delivered => "delivered",
                        },
                        process: match task.process {
                            GlobalTaskProcess::None => "none",
                            GlobalTaskProcess::Running => "running",
                            GlobalTaskProcess::Exited => "exited",
                            GlobalTaskProcess::Unconfirmed => "unconfirmed",
                        },
                    })
                    .collect(),
            })
            .collect();
    }
}
