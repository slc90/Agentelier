use crate::{ActiveTurn, ContextState, ConversationFailure};
use agentelier_llm_model::{ModelFailure, ProtocolHistory};

pub(crate) fn model_error(failure: ModelFailure) -> ConversationFailure {
    ConversationFailure::new(failure.to_string())
}

pub(crate) fn ensure_context_fits(context: &ContextState) -> Result<(), ConversationFailure> {
    if context
        .limits
        .effective
        .or(context.limits.window)
        .is_some_and(|limit| context.tokens > limit)
    {
        return Err(ConversationFailure::new(
            "实际请求材料超过模型有效上下文窗口",
        ));
    }
    Ok(())
}

impl ActiveTurn<'_> {
    pub(crate) fn check_context_capacity(
        &self,
        history: &ProtocolHistory,
    ) -> Result<(), ConversationFailure> {
        let mut context = (*self.view.context).clone();
        context.refresh(
            history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        ensure_context_fits(&context)
    }
}

impl crate::conversation::ChatSession {
    pub(crate) fn check_pending_input(
        &self,
        history: &ProtocolHistory,
        input: agentelier_llm_model::UserMessage,
        transcript: Option<&agentelier_llm_model::TranscriptEvidence>,
    ) -> Result<(), ConversationFailure> {
        if !history.has_open_calls() {
            return Ok(());
        }
        let candidate = history.with_message(input);
        let mut context = (*self.view.context).clone();
        let previous = self
            .view
            .turn
            .as_ref()
            .and_then(|turn| turn.submission.transcript.as_ref())
            .map(|reference| self.view.transcript_evidence(reference))
            .transpose()?;
        context.task_material_tokens = context
            .task_material_tokens
            .saturating_sub(previous.as_ref().map_or(
                0,
                agentelier_llm_model::TranscriptEvidence::estimated_tokens,
            ))
            .saturating_add(transcript.map_or(
                0,
                agentelier_llm_model::TranscriptEvidence::estimated_tokens,
            ));
        context.refresh(
            &candidate,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        ensure_context_fits(&context)
    }
}
