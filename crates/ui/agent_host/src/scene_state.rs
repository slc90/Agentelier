use crate::{AgentHost, MountedScene, SceneState, SceneTarget};
use agentelier_conversation_bridge::SceneStateReceipt;
use bevy::prelude::*;
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

const COALESCE: Duration = Duration::from_millis(175);

#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct SceneSaveError(pub String);

#[derive(Default)]
struct Pending {
    desired: Option<Draft>,
    deadline: Option<Instant>,
    writing: Option<(u64, Draft)>,
    error: Option<(Entity, u64, String)>,
}

struct Draft {
    mount: Entity,
    state: SceneState,
}

#[derive(Resource, Default)]
pub struct SceneStateWrites(HashMap<SceneTarget, Pending>);

impl SceneStateWrites {
    pub fn schedule(&mut self, entity: Entity, mount: MountedScene, state: SceneState) {
        if state.revision != mount.revision {
            return;
        }
        let pending = self.0.entry(mount.target).or_default();
        pending.desired = Some(Draft {
            mount: entity,
            state,
        });
        pending.deadline = Some(Instant::now() + COALESCE);
    }

    fn flush(&mut self, host: &AgentHost, force: bool) {
        let now = Instant::now();
        for (target, pending) in &mut self.0 {
            let Some(deadline) = &mut pending.deadline else {
                continue;
            };
            if force {
                *deadline = now;
            }
            if *deadline > now || pending.writing.is_some() {
                continue;
            }
            let Some(draft) = pending.desired.take() else {
                continue;
            };
            pending.deadline = None;
            match host.save_scene_state(*target, draft.state.clone()) {
                Ok(operation) => pending.writing = Some((operation, draft)),
                Err(error) => {
                    Self::failed(*target, pending, &draft, error.to_string());
                    pending.desired = Some(draft);
                }
            }
        }
    }

    pub fn prepare_exit(&mut self, host: &AgentHost) -> bool {
        self.flush(host, true);
        self.0.values().all(|pending| pending.writing.is_none())
    }

    pub(crate) fn receive(&mut self, operation: u64, receipt: SceneStateReceipt) {
        let target = receipt.target;
        let Some(pending) = self.0.get_mut(&target) else {
            return;
        };
        if !pending
            .writing
            .as_ref()
            .is_some_and(|(id, draft)| *id == operation && draft.state.revision == receipt.revision)
        {
            return;
        }
        let Some((_, submitted)) = pending.writing.take() else {
            return;
        };
        match receipt.result {
            Ok(_) => {
                if pending
                    .desired
                    .as_ref()
                    .is_some_and(|draft| draft.state == submitted.state)
                {
                    pending.desired = None;
                    pending.deadline = None;
                }
                pending.error = None;
            }
            Err(message) => {
                Self::failed(target, pending, &submitted, message);
                pending.desired.get_or_insert(submitted);
            }
        }
        if pending.desired.is_none() && pending.error.is_none() {
            self.0.remove(&target);
        }
    }

    pub(crate) fn connection_failed(&mut self, reason: &str) {
        for (target, pending) in &mut self.0 {
            let writing = pending.writing.take().map(|(_, draft)| draft);
            if let Some(draft) = pending.desired.take().or(writing) {
                Self::failed(*target, pending, &draft, reason.to_owned());
                pending.desired = Some(draft);
            }
            pending.deadline = None;
        }
    }

    fn failed(target: SceneTarget, pending: &mut Pending, draft: &Draft, message: String) {
        let revision = draft.state.revision;
        tracing::warn!(
            session_id = %target.session,
            artifact_id = %target.artifact,
            revision,
            error = %message,
            "Scene 用户状态未保存"
        );
        pending.error = Some((draft.mount, revision, message));
    }
}

pub fn flush_scene_state(world: &mut World) {
    let host = world.resource::<AgentHost>().clone();
    world.resource_mut::<SceneStateWrites>().flush(&host, true);
}

pub(crate) fn update(
    host: Res<AgentHost>,
    mut writes: ResMut<SceneStateWrites>,
    mounted: Query<(Entity, &MountedScene, Option<&SceneSaveError>)>,
    mut commands: Commands,
) {
    writes.flush(&host, false);
    writes.0.retain(|_, pending| {
        pending.deadline.is_some()
            || pending.writing.is_some()
            || pending.error.as_ref().is_some_and(|(entity, revision, _)| {
                mounted
                    .get(*entity)
                    .is_ok_and(|(_, mount, _)| mount.revision == *revision)
            })
    });
    for (entity, mount, displayed) in &mounted {
        let error = writes
            .0
            .get(&mount.target)
            .and_then(|pending| pending.error.as_ref())
            .filter(|(origin, revision, _)| *origin == entity && *revision == mount.revision)
            .map(|(_, _, error)| error);
        if displayed.map(|error| &error.0) == error {
            continue;
        }
        let mut entity = commands.entity(entity);
        if let Some(error) = error {
            entity.try_insert(SceneSaveError(error.clone()));
        } else {
            entity.try_remove::<SceneSaveError>();
        }
    }
}
