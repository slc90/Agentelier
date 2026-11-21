use super::{AgentId, AgentRegistry, AgentRegistryError, AgentState, SessionId, TurnId};
use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct AgentStopSource {
    pub caller: AgentId,
    pub turn: TurnId,
    pub call_id: String,
}

impl std::fmt::Display for AgentStopSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{}:{}:{}",
            self.caller, self.turn.0, self.call_id
        )
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AgentStop {
    pub source: AgentStopSource,
    pub target: AgentId,
    pub operation: Option<AgentStopSource>,
    pub affected: Vec<AgentId>,
}

#[derive(Clone, Default)]
pub(super) struct Stops {
    requests: HashMap<SessionId, HashMap<AgentStopSource, AgentStop>>,
    latest: HashMap<AgentId, AgentStopSource>,
    trees: HashMap<AgentId, AgentStopSource>,
}

impl Stops {
    pub(super) fn retire(&mut self, session: SessionId) {
        if let Some(requests) = self.requests.remove(&session) {
            for stop in requests.into_values() {
                self.trees.remove(&stop.target);
                for agent in stop.affected {
                    self.latest.remove(&agent);
                }
            }
        }
    }
}

impl AgentRegistry {
    pub fn stop_request(&self, session: SessionId, source: &AgentStopSource) -> Option<&AgentStop> {
        self.stops.requests.get(&session)?.get(source)
    }

    pub fn latest_stop(&self, agent: AgentId) -> Option<&AgentStopSource> {
        self.stops.latest.get(&agent)
    }

    pub fn subtree_stopping(&self, session: SessionId, root: AgentId) -> bool {
        self.stops
            .trees
            .get(&root)
            .and_then(|source| self.stop_request(session, source))
            .is_some_and(|request| {
                request.affected.iter().any(|&agent| {
                    self.latest_stop(agent) == Some(&request.source)
                        && self
                            .get(session, agent)
                            .is_ok_and(|node| node.state == AgentState::Stopping)
                })
            })
    }

    pub fn apply_stop(
        &mut self,
        session: SessionId,
        stop: AgentStop,
    ) -> Result<(), AgentRegistryError> {
        for &agent in &stop.affected {
            self.set_state(session, agent, AgentState::Stopping)?;
        }
        self.remember_stop(session, stop);
        Ok(())
    }

    pub(crate) fn remember_stop(&mut self, session: SessionId, stop: AgentStop) {
        if !stop.affected.is_empty() {
            self.stops.trees.insert(stop.target, stop.source.clone());
        }
        for &agent in &stop.affected {
            self.stops.latest.insert(agent, stop.source.clone());
        }
        self.stops
            .requests
            .entry(session)
            .or_default()
            .insert(stop.source.clone(), stop);
    }
}
