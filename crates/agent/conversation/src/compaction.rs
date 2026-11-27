use crate::{
    ActiveTurn, ContextState, ConversationFailure, RecordEvent, SessionId, StoredState, TurnId,
};
use agentelier_llm_model::{CompactionMetadata, ProtocolArchive, ProtocolHistory};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    #[serde(default)]
    pub(crate) project_publication: Option<u64>,
    pub(crate) session: SessionId,
    pub(crate) turn: TurnId,
    pub(crate) request: u32,
    pub(crate) source_window: u64,
    pub(crate) window: u64,
    pub(crate) source_sequence: u64,
    pub(crate) covered_items: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prefix_hash: Option<String>,
    pub(crate) input_retained: bool,
    pub(crate) model: String,
    pub(crate) comp_hash: Option<String>,
    pub(crate) response_id: String,
    pub(crate) compatibility: serde_json::Value,
    pub(crate) history: ProtocolArchive,
}
pub struct CompactionCandidate {
    checkpoint: CompactionCheckpoint,
    history: ProtocolHistory,
}
impl CompactionCandidate {
    fn replacement(
        &self,
        active: &ActiveTurn<'_>,
    ) -> Result<(ProtocolHistory, Box<ContextState>), ConversationFailure> {
        let hash = self
            .checkpoint
            .prefix_hash
            .as_deref()
            .ok_or_else(|| ConversationFailure::new("候选压缩缺少历史前缀摘要"))?;
        let actual = active
            .history
            .archive()
            .prefix_fingerprint(self.checkpoint.covered_items)
            .map_err(crate::result_budget::model_error)?;
        if actual != hash {
            return Err(ConversationFailure::new(
                "已提交压缩的历史前缀已变化，请重新加载存档",
            ));
        }
        let history = active
            .history
            .replace_prefix(self.checkpoint.covered_items, &self.history)
            .map_err(crate::result_budget::model_error)?;
        let mut context = Box::new(ContextState {
            task_material_tokens: active.view.context.task_material_tokens,
            window: self.checkpoint.window,
            limits: active.view.context.limits.clone(),
            latest: active.view.context.latest.clone(),
            ..Default::default()
        });
        context.refresh(
            &history,
            &active.view.settings.instructions,
            active.tools.definitions(),
        )?;
        Ok((history, context))
    }

    pub fn event(&self, active: &ActiveTurn<'_>) -> Result<RecordEvent, ConversationFailure> {
        let mut state = active.stored_state();
        let (history, context) = self.replacement(active)?;
        state.context = context;
        state.committed_items = history.summary();
        if let Some(turn) = &mut state.turn {
            turn.phase = crate::TaskPhase::CompactingContext;
        }
        Ok(RecordEvent::Compacted {
            state,
            checkpoint: Box::new(self.checkpoint.clone()),
        })
    }
}
impl ActiveTurn<'_> {
    pub fn admission_sequence(&self) -> Option<u64> {
        self.state.source_sequence
    }

    pub fn record_admission(&mut self, sequence: u64) {
        self.recorded(sequence);
        self.state.admission = None;
        if self.view.turn.as_ref().is_some_and(|turn| {
            !turn.is_retry
                && turn
                    .submission
                    .uses_audio_draft(self.view.audio_draft.as_ref())
        }) {
            self.view.audio_draft = None;
        }
    }

    pub fn recorded(&mut self, sequence: u64) {
        self.state.source_sequence = Some(sequence);
    }
    pub fn history_confirmed(&self) -> bool {
        self.state.history_confirmed
    }
    pub fn begin_history_commit(&mut self) {
        self.state.history_confirmed = false;
    }
    pub fn confirm_history_commit(&mut self, sequence: u64) {
        self.state.history_confirmed = true;
        self.recorded(sequence);
    }
    pub fn compaction_candidate(
        &mut self,
        result: CompactionMetadata,
    ) -> Result<CompactionCandidate, ConversationFailure> {
        let source_sequence = self
            .state
            .source_sequence
            .ok_or_else(|| ConversationFailure::new("压缩缺少已提交的历史边界"))?;
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("压缩缺少活动任务"))?;
        if !self.state.history_confirmed || !self.state.input_retained {
            return Err(ConversationFailure::new("压缩缺少已确认的响应边界"));
        }
        turn.phase = crate::TaskPhase::CompactingContext;
        let window = self
            .view
            .context
            .window
            .checked_add(1)
            .ok_or_else(|| ConversationFailure::new("工作窗口序号已耗尽"))?;
        let history = self.history.compacted();
        let prefix_hash = self
            .history
            .archive()
            .prefix_fingerprint(self.history.len())
            .map_err(crate::result_budget::model_error)?;
        let checkpoint = CompactionCheckpoint {
            project_publication: self
                .view
                .project_publications
                .last()
                .map(|publication| publication.id),
            session: self.view.id,
            turn: turn.id,
            request: turn.request_index,
            source_window: self.view.context.window,
            window,
            source_sequence,
            covered_items: self.history.len(),
            prefix_hash: Some(prefix_hash),
            input_retained: self.state.input_retained,
            model: self.view.settings.slug.clone(),
            comp_hash: self.view.context.limits.comp_hash.clone(),
            response_id: result.response_id,
            compatibility: result.compatibility,
            history: history.archive(),
        };
        Ok(CompactionCandidate {
            checkpoint,
            history,
        })
    }
    pub fn install_compaction(
        &mut self,
        candidate: CompactionCandidate,
    ) -> Result<(), ConversationFailure> {
        let current = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("压缩安装缺少任务"))?;
        let checkpoint = &candidate.checkpoint;
        if checkpoint.session != self.view.id
            || checkpoint.turn != current.id
            || checkpoint.request != current.request_index
            || checkpoint.source_window != self.view.context.window
            || self
                .state
                .source_sequence
                .is_none_or(|sequence| sequence < checkpoint.source_sequence)
            || checkpoint.input_retained != self.state.input_retained
            || checkpoint.model != self.view.settings.slug
            || checkpoint.comp_hash != self.view.context.limits.comp_hash
        {
            return Err(ConversationFailure::new(
                "已提交压缩检查点与当前任务不一致，请重新加载存档",
            ));
        }
        let (history, context) = candidate.replacement(self)?;
        *self.history = history;
        self.view.context = context;
        self.view.committed_items = self.history.summary();
        self.state.history_confirmed = true;
        Ok(())
    }
}
impl CompactionCheckpoint {
    pub(crate) fn validate(&self, state: &StoredState) -> Result<(), ConversationFailure> {
        self.history
            .validate_compaction()
            .map_err(|failure| ConversationFailure::new(failure.to_string()))?;
        if self.session != state.session
            || state.turn.as_ref().is_none_or(|turn| {
                turn.id != self.turn
                    || turn.request_index != self.request
                    || !matches!(turn.phase, crate::TaskPhase::CompactingContext)
                    || !matches!(turn.status, crate::TurnStatus::Running)
            })
            || self.source_window.checked_add(1) != Some(self.window)
            || state.context.window != self.window
            || self.response_id.is_empty()
            || self.source_sequence == 0
            || self.comp_hash != state.context.limits.comp_hash
        {
            return Err(ConversationFailure::new("压缩检查点身份或窗口不一致"));
        }
        Ok(())
    }
}
