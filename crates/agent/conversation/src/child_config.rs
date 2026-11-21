use crate::{ActiveTurn, ConversationFailure, SessionId, StoredSettings, TurnId};
use agentelier_llm_model::{
    CodexIdentity, ModelInfo, ProjectPublication, ProtocolHistory, codex_tool_contract,
    select_agent_model,
};
use agentelier_prompts::RoleSnapshot;
use agentelier_tools::ToolCatalog;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForkMode {
    FullHistory,
    #[default]
    FreshContext,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ForkBoundary {
    pub mode: ForkMode,
    pub parent_thread: SessionId,
    pub turn: TurnId,
    pub request: u32,
    pub call_id: String,
    pub window: u64,
    pub items: usize,
    pub history_hash: String,
    pub parent_request_hash: String,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildExecutionPolicy {
    DesktopUserWithoutToolApproval,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ChildConfigSnapshot {
    pub fork: ForkBoundary,
    pub settings: StoredSettings,
    pub execution: ChildExecutionPolicy,
    pub role: RoleSnapshot,
    pub projects: Vec<ProjectPublication>,
    pub skills: agentelier_skills::Catalog,
}

pub struct PreparedChild {
    pub config: ChildConfigSnapshot,
    pub history: ProtocolHistory,
    pub tools: Arc<ToolCatalog>,
}

pub struct ChildPreparation<'a> {
    pub request: u32,
    pub call_id: &'a str,
    pub fork: ForkMode,
    pub model: Option<&'a str>,
    pub effort: Option<&'a str>,
    pub role: &'a RoleSnapshot,
}

impl ActiveTurn<'_> {
    pub fn prepare_child(
        &self,
        options: ChildPreparation<'_>,
        models: &[ModelInfo],
        identity: &CodexIdentity,
    ) -> Result<PreparedChild, ConversationFailure> {
        let parent = &self.view.settings;
        parent.validate_source(identity)?;
        let turn = self
            .view
            .turn
            .as_ref()
            .ok_or_else(|| ConversationFailure::new("准备子 Agent 缺少活动轮次"))?;
        if !self.history_confirmed()
            || !turn.tools.iter().any(|tool| {
                tool.request_index == options.request && tool.call_id == options.call_id
            })
        {
            return Err(ConversationFailure::new(
                "准备子 Agent 缺少已确认的来源调用",
            ));
        }
        let request = turn
            .requests
            .iter()
            .find(|request| {
                request.request == options.request && request.response_accepted == Some(true)
            })
            .ok_or_else(|| ConversationFailure::new("准备子 Agent 缺少已确认的冻结请求"))?;
        let mut projects: Vec<_> = self
            .view
            .project_publications
            .iter()
            .take_while(|publication| Some(publication.id) <= request.publication)
            .map(|publication| publication.as_ref().clone())
            .collect();
        let project = projects
            .last()
            .filter(|project| Some(project.id) == request.publication)
            .ok_or_else(|| ConversationFailure::new("准备子 Agent 缺少调用步骤的项目快照"))?;
        let history = match options.fork {
            ForkMode::FullHistory => {
                if request.window != self.view.context.window {
                    return Err(ConversationFailure::new(
                        "压缩已替换 Full History Fork 的请求边界",
                    ));
                }
                self.history
                    .fork_at(&request.projection.items)
                    .map_err(failure)?
            }
            ForkMode::FreshContext => {
                let project = ProjectPublication::initial(1, 0, project.snapshot.clone());
                let mut history = ProtocolHistory::default();
                if let Some(item) = &project.item {
                    history = history.with_project(item.clone());
                }
                projects = vec![project];
                history
            }
        };
        for project in &projects {
            project.validate().map_err(failure)?;
        }
        let publications = projects.iter().cloned().map(Arc::new).collect::<Vec<_>>();
        history.validate_projects(&publications).map_err(failure)?;

        let model = options.model.or(options.role.model.as_deref());
        let effort = options.effort.or(options.role.reasoning.as_deref());
        let (slug, display_name, effort, model_image, limits) =
            if model.is_some() || effort.is_some() {
                let (selected, effort) =
                    select_agent_model(models, model.unwrap_or(&parent.slug), effort)
                        .map_err(failure)?;
                (
                    &selected.slug,
                    &selected.display_name,
                    effort,
                    selected.image_input,
                    &selected.context_limits,
                )
            } else {
                (
                    &parent.slug,
                    &parent.display_name,
                    parent.effort.as_deref(),
                    parent.image_input,
                    &self.view.context.limits,
                )
            };
        if !limits.available() {
            return Err(ConversationFailure::new("子 Agent 模型缺少有效上下文限制"));
        }
        let image_input = match (parent.image_input, model_image) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        };
        if image_input == Some(false) && history.image_bytes_with(std::iter::empty()) != 0 {
            return Err(ConversationFailure::new(
                "子 Agent 无法重放含图片的 Fork 历史",
            ));
        }
        let parent_tools: BTreeSet<_> = self
            .tools
            .definitions()
            .map(|tool| tool.name.clone())
            .collect();
        let allowed = options.role.narrow_tools(&parent_tools).map_err(failure)?;
        let tools = Arc::new(self.tools.inherited(
            &allowed,
            codex_tool_contract(slug),
            image_input,
        ));
        let instructions = parent
            .instructions
            .for_child(options.role)
            .map_err(failure)?;
        let skills =
            turn.skills.as_deref().cloned().ok_or_else(|| {
                ConversationFailure::new("准备子 Agent 缺少调用步骤的 Skills 目录")
            })?;
        let config = ChildConfigSnapshot {
            fork: ForkBoundary {
                mode: options.fork,
                parent_thread: self.view.id,
                turn: turn.id,
                request: options.request,
                call_id: options.call_id.into(),
                window: request.window,
                items: history.position(),
                history_hash: history
                    .archive()
                    .prefix_fingerprint(history.position())
                    .map_err(failure)?,
                parent_request_hash: request.projection.payload_hash.clone(),
            },
            settings: StoredSettings {
                codex: Some(identity.clone()),
                memory: None,
                context_limits: Box::new(limits.clone()),
                slug: slug.clone(),
                display_name: display_name.clone(),
                effort: effort.map(str::to_owned),
                image_input,
                instructions,
                working_directory: parent.working_directory.clone(),
                tool_contract: tools.contract(),
                tools: tools.definitions().cloned().collect(),
            },
            execution: ChildExecutionPolicy::DesktopUserWithoutToolApproval,
            role: options.role.clone(),
            projects,
            skills,
        };
        Ok(PreparedChild {
            config,
            history,
            tools,
        })
    }
}

fn failure(error: impl std::fmt::Display) -> ConversationFailure {
    ConversationFailure::new(error.to_string())
}
