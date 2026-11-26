use crate::{
    ConversationFailure, RecordEvent, SessionRecord, StoredState, ToolView, TurnStatus, TurnView,
};
use agentelier_tools::{ProcessChange, ProcessCleanup, ToolStatus};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub struct MemorySourceTurn {
    pub input_sequence: u64,
    pub evidence_sequence: u64,
    pub activity_ms: u64,
    pub finished_sequence: u64,
    pub work_directory: PathBuf,
    pub tool_sequences: BTreeMap<String, u64>,
    pub turn: Box<TurnView>,
}

#[derive(Default)]
struct Evidence {
    input_sequence: Option<u64>,
    evidence_sequence: u64,
    activity_ms: u64,
    finished_sequence: u64,
    work_directory: Option<PathBuf>,
    finished: Option<Box<TurnView>>,
    calls: BTreeMap<String, Option<(u64, ToolView)>>,
    processes: BTreeMap<String, ProcessEvidence>,
}

enum ProcessEvidence {
    Running,
    Exited(ProcessCleanup),
    Historical(ProcessCleanup),
}

impl ProcessEvidence {
    fn closed(&self) -> bool {
        match self {
            Self::Running => false,
            Self::Exited(exit) => exit.cleanup_confirmed,
            Self::Historical(_) => true,
        }
    }
}

impl Evidence {
    fn observe(&mut self, sequence: u64, at_ms: u64) {
        self.evidence_sequence = sequence;
        self.activity_ms = self.activity_ms.max(at_ms);
    }

    fn install_returns(&mut self, turn: &TurnView, sequence: u64) {
        for tool in &turn.tools {
            if tool.retained && terminal(tool.status) {
                let saved = self.calls.entry(tool.call_id.clone()).or_default();
                if saved
                    .as_ref()
                    .is_none_or(|(_, last)| last.status != tool.status)
                {
                    *saved = Some((sequence, tool.clone()));
                }
            }
        }
    }

    fn close(mut self) -> Option<MemorySourceTurn> {
        let mut turn = self.finished?;
        let input_sequence = self.input_sequence?;
        if self.calls.values().any(Option::is_none)
            || self.processes.values().any(|exit| !exit.closed())
        {
            return None;
        }
        let mut tool_sequences = BTreeMap::new();
        for tool in &mut turn.tools {
            if let Some(Some((sequence, saved))) = self.calls.remove(&tool.call_id) {
                tool_sequences.insert(tool.call_id.clone(), sequence);
                *tool = saved;
            }
        }
        for (id, (sequence, saved)) in self
            .calls
            .into_iter()
            .filter_map(|(id, saved)| saved.map(|saved| (id, saved)))
        {
            tool_sequences.insert(id, sequence);
            turn.tools.push(saved);
        }
        for exit in self
            .processes
            .into_values()
            .filter_map(|process| match process {
                ProcessEvidence::Exited(exit) | ProcessEvidence::Historical(exit) => Some(exit),
                ProcessEvidence::Running => None,
            })
        {
            if let Some(saved) = turn
                .process_cleanup
                .iter_mut()
                .find(|saved| saved.session_id == exit.session_id)
            {
                *saved = exit;
            } else {
                turn.process_cleanup.push(exit);
            }
        }
        Some(MemorySourceTurn {
            input_sequence,
            evidence_sequence: self.evidence_sequence,
            activity_ms: self.activity_ms,
            finished_sequence: self.finished_sequence,
            work_directory: self.work_directory?,
            tool_sequences,
            turn,
        })
    }
}

fn terminal(status: ToolStatus) -> bool {
    !matches!(
        status,
        ToolStatus::Receiving | ToolStatus::Pending | ToolStatus::Running
    )
}

fn turns(state: &StoredState) -> impl Iterator<Item = &TurnView> {
    state
        .turn
        .iter()
        .map(Box::as_ref)
        .chain(&state.previous_turns)
}

pub fn memory_evidence(
    records: Vec<SessionRecord>,
    mut work_directory: PathBuf,
) -> Result<Vec<MemorySourceTurn>, ConversationFailure> {
    let mut sources = BTreeMap::<u64, Evidence>::new();
    for record in records {
        let sequence = record.sequence;
        let at_ms = record.at_ms;
        match record.event {
            RecordEvent::WorkingDirectoryChanged { directory, .. } => work_directory = directory,
            RecordEvent::Accepted { state }
            | RecordEvent::ContinuedWithoutCompaction { state, .. } => {
                if let Some(turn) = state.turn {
                    let source = sources.entry(turn.id.0).or_default();
                    source.input_sequence.get_or_insert(sequence);
                    source
                        .work_directory
                        .get_or_insert_with(|| work_directory.clone());
                    source.finished = None;
                    source.observe(sequence, at_ms);
                }
            }
            RecordEvent::Response { state, .. } => {
                if let Some(turn) = state.turn {
                    let source = sources.entry(turn.id.0).or_default();
                    for call in turn
                        .tools
                        .into_iter()
                        .filter(|tool| tool.retained && tool.request_index == turn.request_index)
                    {
                        source.calls.entry(call.call_id).or_insert(None);
                    }
                    source.observe(sequence, at_ms);
                }
            }
            RecordEvent::ToolReturned {
                state,
                call_id,
                status,
                ..
            } => {
                let (turn, tool) = turns(&state)
                    .find_map(|turn| {
                        turn.tools
                            .iter()
                            .find(|tool| tool.call_id == call_id)
                            .map(|tool| (turn, tool))
                    })
                    .ok_or_else(|| ConversationFailure::new("记忆工具结果缺少持久来源"))?;
                if !terminal(status) || tool.status != status {
                    return Err(ConversationFailure::new("记忆工具结果状态与持久来源不一致"));
                }
                let source = sources.entry(turn.id.0).or_default();
                let mut saved = tool.clone();
                saved.retained = true;
                source.calls.insert(call_id, Some((sequence, saved)));
                source.observe(sequence, at_ms);
            }
            RecordEvent::Process { fact } => {
                let source = sources.entry(fact.origin.turn).or_default();
                match fact.change {
                    ProcessChange::Started { .. } => {
                        source
                            .processes
                            .insert(fact.session_id, ProcessEvidence::Running);
                    }
                    ProcessChange::Exited {
                        cleanup_confirmed,
                        output,
                        ..
                    } => {
                        if source
                            .processes
                            .insert(
                                fact.session_id.clone(),
                                ProcessEvidence::Exited(ProcessCleanup {
                                    session_id: fact.session_id,
                                    cleanup_confirmed,
                                    output,
                                }),
                            )
                            .is_none()
                        {
                            return Err(ConversationFailure::new("记忆进程退出缺少持久来源"));
                        }
                    }
                    ProcessChange::Consumed => continue,
                }
                source.observe(sequence, at_ms);
            }
            RecordEvent::Sealed { state, .. } => {
                if let Some(turn) = state.turn {
                    let source = sources.entry(turn.id.0).or_default();
                    source.install_returns(&turn, sequence);
                    source.observe(sequence, at_ms);
                }
            }
            RecordEvent::Finished { state, .. } => {
                if let Some(turn) = state.turn
                    && !matches!(turn.status, TurnStatus::Running)
                {
                    let source = sources.entry(turn.id.0).or_default();
                    source.finished = Some(turn);
                    source.finished_sequence = sequence;
                    source.observe(sequence, at_ms);
                }
            }
            RecordEvent::Recovered { state, .. } => {
                for turn in state
                    .previous_turns
                    .into_iter()
                    .map(Box::new)
                    .chain(state.turn)
                {
                    let source = sources.entry(turn.id.0).or_default();
                    let mut changed =
                        source.finished.is_none() || source.calls.values().any(Option::is_none);
                    for (id, resource) in &mut source.processes {
                        if !resource.closed() {
                            let output = match resource {
                                ProcessEvidence::Exited(exit) => format!(
                                    "历史清理未确认，当前运行没有可恢复句柄。{}",
                                    exit.output
                                ),
                                _ => "历史进程退出未确认，当前运行没有可恢复句柄".to_owned(),
                            };
                            *resource = ProcessEvidence::Historical(ProcessCleanup {
                                session_id: id.clone(),
                                cleanup_confirmed: false,
                                output,
                            });
                            changed = true;
                        }
                    }
                    if changed {
                        source.install_returns(&turn, sequence);
                        if !matches!(turn.status, TurnStatus::Running) {
                            if source.finished.is_none() {
                                source.finished_sequence = sequence;
                            }
                            source.finished = Some(turn);
                        }
                        source.observe(sequence, at_ms);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(sources.into_values().map_while(Evidence::close).collect())
}
