use crate::ConversationFailure;
use agentelier_tools::{ProcessChange, ProcessFact, ProcessPhase, ProcessView};
use std::collections::BTreeMap;

#[derive(Clone, Default)]
pub struct ProcessLedger(BTreeMap<String, ProcessView>);

impl ProcessLedger {
    pub fn apply(&mut self, fact: &ProcessFact) -> Result<(), ConversationFailure> {
        let failure = || ConversationFailure::new("进程事实与已登记的来源或状态不一致");
        match &fact.change {
            ProcessChange::Started => {
                if self.0.contains_key(&fact.session_id)
                    || uuid::Uuid::parse_str(&fact.session_id).is_err()
                {
                    return Err(failure());
                }
                self.0
                    .insert(fact.session_id.clone(), ProcessView::registered(fact));
            }
            change => {
                let view = self
                    .0
                    .get_mut(&fact.session_id)
                    .filter(|view| view.origin == fact.origin)
                    .ok_or_else(failure)?;
                let valid = match change {
                    ProcessChange::Exited { .. } => view.phase == ProcessPhase::Running,
                    ProcessChange::Consumed => view.phase == ProcessPhase::Exited && view.readable,
                    ProcessChange::Started => false,
                };
                if !valid {
                    return Err(failure());
                }
                view.apply(change);
            }
        }
        Ok(())
    }

    pub fn historical(&mut self) {
        for process in self.0.values_mut() {
            if matches!(
                process.phase,
                ProcessPhase::Running | ProcessPhase::CleanupUnconfirmed
            ) {
                process.phase = ProcessPhase::HistoricalUnconfirmed;
            }
            process.readable = false;
        }
    }

    pub fn views(&self) -> impl Iterator<Item = &ProcessView> {
        self.0.values()
    }

    pub fn has_active(&self) -> bool {
        self.views().any(|process| {
            matches!(
                process.phase,
                ProcessPhase::Running | ProcessPhase::CleanupUnconfirmed
            )
        })
    }
}
