use crate::{
    ActiveTurn, ContextState, ConversationFailure, RecordEvent, SessionId, StoredState, TurnId,
    UsageFact,
};
use agentelier_llm_model::{CompactionResult, GenerationRequest, ProtocolArchive, ProtocolHistory};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct CompactionCheckpoint {
    pub(crate) session: SessionId,
    pub(crate) turn: TurnId,
    pub(crate) request: u32,
    pub(crate) source_window: u64,
    pub(crate) window: u64,
    pub(crate) source_sequence: u64,
    pub(crate) covered_items: usize,
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
    context: Box<ContextState>,
}
impl CompactionCandidate {
    pub fn event(&self, active: &ActiveTurn) -> RecordEvent {
        let mut state = active.stored_state();
        state.context = self.context.clone();
        state.committed_items = self.history.summary();
        RecordEvent::Compacted {
            state,
            checkpoint: Box::new(self.checkpoint.clone()),
        }
    }
}
impl ActiveTurn {
    pub fn recorded(&mut self, sequence: u64) {
        self.source_sequence = Some(sequence);
    }
    pub fn history_confirmed(&self) -> bool {
        self.history_confirmed
    }
    pub fn committing_checkpoint(&mut self) {
        self.history_confirmed = false;
    }
    pub fn context_reached(&mut self) -> Result<bool, ConversationFailure> {
        let history = self.next_context()?;
        self.view
            .context
            .refresh(&history, &self.view.settings.instructions, &self.tools)?;
        Ok(self.view.context.limits.reached(self.view.context.tokens))
    }
    fn next_context(&self) -> Result<ProtocolHistory, ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("上下文缺少活动任务"))?;
        Ok(if self.input_retained {
            self.history.clone()
        } else {
            self.history.with_user(&turn.input)
        })
    }
    pub fn compaction_request(&mut self) -> Result<GenerationRequest, ConversationFailure> {
        if !self.results.is_empty() || self.history.is_empty() {
            return Err(ConversationFailure::new(
                "当前输入超出上下文预算，缺少可压缩的封闭历史",
            ));
        }
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or_else(|| ConversationFailure::new("压缩缺少活动任务"))?;
        turn.request_index = turn
            .request_index
            .checked_add(1)
            .ok_or_else(|| ConversationFailure::new("模型请求序号已耗尽"))?;
        turn.phase = crate::TaskPhase::CompactingContext;
        turn.request_items = self.history.summary();
        Ok(GenerationRequest {
            slug: self.view.settings.slug.clone(),
            effort: self.view.settings.effort.clone(),
            context: self.history.clone(),
            instructions: self.view.settings.instructions.clone(),
            tools: self.tools.clone(),
        })
    }
    pub fn compaction_candidate(
        &self,
        result: CompactionResult,
    ) -> Result<CompactionCandidate, ConversationFailure> {
        let source_sequence = self
            .source_sequence
            .ok_or_else(|| ConversationFailure::new("压缩缺少已提交的历史边界"))?;
        let turn = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("压缩缺少活动任务"))?;
        if !self.results.is_empty() || !matches!(turn.phase, crate::TaskPhase::CompactingContext) {
            return Err(ConversationFailure::new("压缩不处于有效封闭边界"));
        }
        let window = self
            .view
            .context
            .window
            .checked_add(1)
            .ok_or_else(|| ConversationFailure::new("工作窗口序号已耗尽"))?;
        let history = self.history.compacted(result.item);
        let mut context = Box::new(ContextState {
            window,
            limits: self.view.context.limits.clone(),
            ..Default::default()
        });
        context.latest = result.usage.map(|usage| UsageFact {
            window: self.view.context.window,
            turn: turn.id.0,
            request: turn.request_index,
            response_id: Some(result.response_id.clone()),
            usage,
        });
        let next = if self.input_retained {
            history.clone()
        } else {
            history.with_user(&turn.input)
        };
        context.refresh(&next, &self.view.settings.instructions, &self.tools)?;
        if context.limits.reached(context.tokens) || context.tokens >= self.view.context.tokens {
            return Err(ConversationFailure::new(
                "压缩后仍无法容纳下一请求，保留原窗口并停止本次任务",
            ));
        }
        context.refresh(&history, &self.view.settings.instructions, &self.tools)?;
        let checkpoint = CompactionCheckpoint {
            session: self.view.id,
            turn: turn.id,
            request: turn.request_index,
            source_window: self.view.context.window,
            window,
            source_sequence,
            covered_items: self.history.len(),
            input_retained: self.input_retained,
            model: self.view.settings.slug.clone(),
            comp_hash: context.limits.comp_hash.clone(),
            response_id: result.response_id,
            compatibility: result.compatibility,
            history: history.archive(),
        };
        Ok(CompactionCandidate {
            checkpoint,
            history,
            context,
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
            || Some(checkpoint.source_sequence) != self.source_sequence
            || checkpoint.covered_items != self.history.len()
            || checkpoint.input_retained != self.input_retained
            || checkpoint.model != self.view.settings.slug
            || checkpoint.comp_hash != self.view.context.limits.comp_hash
            || !matches!(current.phase, crate::TaskPhase::CompactingContext)
        {
            return Err(ConversationFailure::new(
                "已提交压缩检查点与当前任务不一致，请重新加载存档",
            ));
        }
        self.history = candidate.history;
        self.view.context = candidate.context;
        self.view.committed_items = self.history.summary();
        self.history_confirmed = true;
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
