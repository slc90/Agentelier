use crate::{ContextState, Conversation, ConversationFailure, SessionId, SessionView};
use agentelier_llm_model::{ContextLimits, ModelInfo, codex_tool_contract, select_session_model};
use agentelier_tools::ToolContract;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct StoredSessionModel {
    pub slug: String,
    pub display_name: String,
    pub image_input: Option<bool>,
    pub context_limits: ContextLimits,
}

pub struct PreparedModelChange {
    pub model: StoredSessionModel,
    context: ContextState,
}

impl Conversation {
    pub fn prepare_model_change(
        &self,
        session: SessionId,
        slug: &str,
        models: &[ModelInfo],
    ) -> Result<Option<PreparedModelChange>, ConversationFailure> {
        let current = self
            .session
            .as_ref()
            .filter(|current| current.view.id == session)
            .ok_or_else(|| ConversationFailure::new("要更改模型的会话已变化"))?;
        if current.view.execution_blocked {
            return Err(ConversationFailure::new("当前会话处于只读状态"));
        }
        if slug == current.view.settings.slug {
            return Ok(None);
        }
        let model = match &current.view.settings.effort {
            Some(effort) => select_session_model(models, slug, effort)
                .map_err(|failure| ConversationFailure::new(failure.to_string()))?,
            None => models
                .iter()
                .find(|model| model.slug == slug)
                .ok_or_else(|| ConversationFailure::new("当前账号模型目录不包含所选模型"))?,
        };
        if current.tools.contract() == ToolContract::CodexWebSocket
            && codex_tool_contract(&model.slug) != ToolContract::CodexWebSocket
        {
            return Err(ConversationFailure::new(
                "所选模型不支持该会话已保存的 Codex WebSocket 工具契约",
            ));
        }
        if !model.context_limits.available() {
            return Err(ConversationFailure::new(
                "所选模型缺少有效的上下文窗口或压缩阈值",
            ));
        }
        let history = current
            .history
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("有模型任务正在使用会话历史，请稍后重试"))?;
        let mut context = current.view.context.as_ref().clone();
        context.limits = model.context_limits.clone();
        context.anchor = None;
        context.refresh(
            history,
            &current.view.settings.instructions,
            current.tools.definitions(),
        )?;
        Ok(Some(PreparedModelChange {
            model: StoredSessionModel {
                slug: model.slug.clone(),
                display_name: model.display_name.clone(),
                image_input: model.image_input,
                context_limits: model.context_limits.clone(),
            },
            context,
        }))
    }

    pub fn install_model_change(
        &mut self,
        session: SessionId,
        change: PreparedModelChange,
    ) -> Result<SessionView, ConversationFailure> {
        let current = self
            .session
            .as_mut()
            .filter(|current| current.view.id == session)
            .ok_or_else(|| ConversationFailure::new("模型保存后无法找到原会话"))?;
        current.view.settings.slug = change.model.slug;
        current.view.settings.display_name = change.model.display_name;
        current.view.settings.image_input = change.model.image_input;
        *current.view.settings.context_limits = change.model.context_limits;
        *current.view.context = change.context;
        Ok(current.view.clone())
    }
}
