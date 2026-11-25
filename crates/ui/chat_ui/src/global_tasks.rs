use crate::archives::SessionActivity;
use agentelier_agent_host::{GlobalTasksEvent, ReceiveAgentEvents};
use agentelier_conversation_bridge::{
    GlobalTaskProcess, GlobalTaskStatus, SessionId, TaskDelivery,
};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Resource, Default)]
pub(crate) struct GlobalTaskState {
    revision: u64,
    activities: HashMap<SessionId, SessionActivity>,
}

impl GlobalTaskState {
    pub(crate) fn activity(&self, session: SessionId) -> SessionActivity {
        self.activities.get(&session).copied().unwrap_or_default()
    }
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GlobalTaskState>()
        .add_systems(Update, receive.after(ReceiveAgentEvents));
}

fn receive(mut events: MessageReader<GlobalTasksEvent>, mut state: ResMut<GlobalTaskState>) {
    for GlobalTasksEvent(snapshot) in events.read() {
        if snapshot.revision <= state.revision {
            continue;
        }
        state.revision = snapshot.revision;
        state.activities = snapshot
            .groups
            .iter()
            .map(|group| {
                let activity = group
                    .tasks
                    .iter()
                    .map(|task| match (task.status, task.delivery, task.process) {
                        (GlobalTaskStatus::NeedsConfirmation, _, _)
                        | (_, _, GlobalTaskProcess::Unconfirmed) => SessionActivity::Error,
                        (GlobalTaskStatus::Failed, TaskDelivery::Delivered, _) => {
                            SessionActivity::Idle
                        }
                        (GlobalTaskStatus::Failed, _, _) => SessionActivity::Error,
                        (
                            GlobalTaskStatus::Waiting
                            | GlobalTaskStatus::Running
                            | GlobalTaskStatus::WaitingForResources
                            | GlobalTaskStatus::Cancelling,
                            _,
                            _,
                        )
                        | (
                            GlobalTaskStatus::ResultReady,
                            TaskDelivery::Saved | TaskDelivery::Appended,
                            _,
                        )
                        | (_, _, GlobalTaskProcess::Running) => SessionActivity::Busy,
                        _ => SessionActivity::Idle,
                    })
                    .max()
                    .unwrap_or_default();
                (group.session, activity)
            })
            .collect();
    }
}
