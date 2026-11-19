use crate::{ActiveTurn, ContextState, ConversationFailure};
use agentelier_llm_model::{
    MAX_REQUEST_BYTES, ModelFailure, ProtocolHistory, result_token_budget, result_tokens,
    result_wire_bytes,
};
use agentelier_tools::{MAX_ENCODED_IMAGE_BYTES, MAX_SESSION_IMAGE_BYTES, MAX_TOOL_BODY_BYTES};
use std::collections::{BTreeMap, HashSet};

// Leave room for a control response and the next request after outstanding results land.
const CONTINUATION_TOKENS: u64 = 1_000;
const CONTINUATION_WIRE_BYTES: usize = 8 * 1024;
const RESULT_ENVELOPE_BYTES: usize = 256;
const REJECTION_TEXT_BYTES: usize = 512;
const MAX_JSON_ESCAPE_BYTES: usize = 6;

pub(crate) fn model_error(failure: ModelFailure) -> ConversationFailure {
    ConversationFailure::new(failure.to_string())
}

#[derive(Default)]
pub(crate) struct ResultHeadroom {
    tokens: u64,
    wire_bytes: usize,
    image_bytes: usize,
}

impl ResultHeadroom {
    pub(crate) fn ensure(
        &self,
        context: &ContextState,
        wire_bytes: usize,
    ) -> Result<(), ConversationFailure> {
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
        if !fits_tokens
            || self.image_bytes > MAX_SESSION_IMAGE_BYTES
            || wire_bytes.saturating_add(self.wire_reserve()) > MAX_REQUEST_BYTES
        {
            return Err(ConversationFailure::new(
                "上下文容量不足，等待后台结果以释放上下文，或保留当前内容后另开会话",
            ));
        }
        Ok(())
    }

    pub(crate) fn wire_reserve(&self) -> usize {
        self.wire_bytes.saturating_add(CONTINUATION_WIRE_BYTES)
    }
}

impl ResultHeadroom {
    fn measure(
        tools: &agentelier_tools::ToolCatalog,
        results: &BTreeMap<String, crate::active::ToolResultEntry>,
        history: &ProtocolHistory,
        rejected: &HashSet<&str>,
    ) -> Result<ResultHeadroom, ConversationFailure> {
        let mut staged = Vec::new();
        let mut room = ResultHeadroom::default();
        for (call_id, name) in history.open_calls() {
            if let Some(result) = results.get(call_id) {
                staged.push(&result.output);
                room.wire_bytes = room.wire_bytes.saturating_add(
                    result_wire_bytes(call_id, &result.output).map_err(model_error)?,
                );
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
                room.wire_bytes = room
                    .wire_bytes
                    .saturating_add(RESULT_ENVELOPE_BYTES)
                    .saturating_add(
                        call_id
                            .len()
                            .saturating_add(text)
                            .saturating_mul(MAX_JSON_ESCAPE_BYTES),
                    )
                    .saturating_add(encoded.div_ceil(3).saturating_mul(4));
            }
        }
        room.image_bytes = room
            .image_bytes
            .saturating_add(history.image_bytes_with(staged.into_iter()));
        Ok(room)
    }
}
impl ActiveTurn<'_> {
    pub(crate) fn check_result_capacity(
        &self,
        history: &ProtocolHistory,
        fixed_bytes: usize,
        rejected: &HashSet<&str>,
        new_window: bool,
    ) -> Result<ResultHeadroom, ConversationFailure> {
        let mut context = (*self.view.context).clone();
        if new_window {
            context.anchor = None;
        }
        context.refresh(
            history,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        let room = ResultHeadroom::measure(self.tools, self.results, history, rejected)?;
        room.ensure(
            &context,
            fixed_bytes.saturating_add(history.input_wire_bytes().map_err(model_error)?),
        )?;
        Ok(room)
    }
}

impl crate::conversation::ChatSession {
    pub(crate) fn check_pending_input(
        &self,
        history: &ProtocolHistory,
        input: &str,
    ) -> Result<(), ConversationFailure> {
        if !history.has_open_calls() {
            return Ok(());
        }
        let candidate = history.with_user(input);
        let mut context = (*self.view.context).clone();
        context.refresh(
            &candidate,
            &self.view.settings.instructions,
            self.tools.definitions(),
        )?;
        let fixed = self
            .view
            .turn
            .as_ref()
            .and_then(|turn| turn.requests.last())
            .and_then(|request| request.projection.fixed_bytes)
            .unwrap_or_default();
        ResultHeadroom::measure(
            &self.tools,
            &self.pending_results,
            &candidate,
            &HashSet::new(),
        )?
        .ensure(
            &context,
            fixed.saturating_add(candidate.input_wire_bytes().map_err(model_error)?),
        )
    }
}
