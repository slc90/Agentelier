use crate::{ActiveTurn, ContextState, ConversationFailure};
use agentelier_llm_model::{ModelFailure, ProtocolHistory, result_token_budget, result_tokens};
use agentelier_tools::{MAX_ENCODED_IMAGE_BYTES, MAX_SESSION_IMAGE_BYTES, MAX_TOOL_BODY_BYTES};
use std::collections::{BTreeMap, HashSet};

// Leave room for a control response and the next request after outstanding results land.
const CONTINUATION_TOKENS: u64 = 1_000;
const REJECTION_TEXT_BYTES: usize = 512;

pub(crate) fn model_error(failure: ModelFailure) -> ConversationFailure {
    ConversationFailure::new(failure.to_string())
}

#[derive(Default)]
pub(crate) struct ResultHeadroom {
    tokens: u64,
    image_bytes: usize,
}

impl ResultHeadroom {
    pub(crate) fn ensure(&self, context: &ContextState) -> Result<(), ConversationFailure> {
        let fits_tokens = context
            .limits
            .effective
            .or(context.limits.window)
            .or(context.limits.automatic)
            .is_none_or(|limit| {
                context
                    .tokens
                    .saturating_add(self.tokens)
                    .saturating_add(CONTINUATION_TOKENS)
                    <= limit
            });
        if !fits_tokens || self.image_bytes > MAX_SESSION_IMAGE_BYTES {
            return Err(ConversationFailure::new(
                "上下文容量不足，等待后台结果以释放上下文，或保留当前内容后另开会话",
            ));
        }
        Ok(())
    }

    fn measure(
        tools: &agentelier_tools::ToolCatalog,
        results: &BTreeMap<String, crate::active::ToolResultEntry>,
        history: &ProtocolHistory,
        rejected: &HashSet<&str>,
    ) -> Self {
        let mut staged = Vec::new();
        let mut room = ResultHeadroom::default();
        for (call_id, name) in history.open_calls() {
            if let Some(result) = results.get(call_id) {
                staged.push(&result.output);
                room.tokens = room.tokens.saturating_add(result_tokens(
                    call_id,
                    result.output.summary(),
                    result.output.image().is_some(),
                ));
            } else {
                let rejected = rejected.contains(call_id);
                let text = if rejected {
                    REJECTION_TEXT_BYTES
                } else {
                    MAX_TOOL_BODY_BYTES
                };
                let image = !rejected && tools.may_return_image(name);
                let encoded = if image { MAX_ENCODED_IMAGE_BYTES } else { 0 };
                room.image_bytes = room.image_bytes.saturating_add(encoded);
                room.tokens = room
                    .tokens
                    .saturating_add(result_token_budget(call_id, text, image));
            }
        }
        room.image_bytes = room
            .image_bytes
            .saturating_add(history.image_bytes_with(staged.into_iter()));
        room
    }
}
impl ActiveTurn<'_> {
    pub(crate) fn check_result_capacity(
        &self,
        history: &ProtocolHistory,
        rejected: &HashSet<&str>,
        new_window: bool,
    ) -> Result<(), ConversationFailure> {
        let mut context = (*self.view.context).clone();
        if new_window {
            context.anchor = None;
        }
        context.refresh(
            history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        ResultHeadroom::measure(self.tools, self.results, history, rejected).ensure(&context)
    }
}

impl crate::conversation::ChatSession {
    pub(crate) fn check_pending_input(
        &self,
        history: &ProtocolHistory,
        input: &str,
        transcript: Option<&agentelier_llm_model::TranscriptEvidence>,
    ) -> Result<(), ConversationFailure> {
        if !history.has_open_calls() {
            return Ok(());
        }
        let candidate = history.with_user(input);
        let mut context = (*self.view.context).clone();
        let previous = self
            .view
            .turn
            .as_ref()
            .and_then(|turn| turn.transcript.as_ref())
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
        ResultHeadroom::measure(
            &self.tools,
            &self.pending_results,
            &candidate,
            &HashSet::new(),
        )
        .ensure(&context)
    }
}
