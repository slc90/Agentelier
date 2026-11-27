use agentelier_llm_model::{ProjectPublication, ProtocolHistory, RequestProjection};
use agentelier_project_context::ProjectSnapshot;
use std::sync::Arc;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct PreparedRequestFact {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response_accepted: Option<bool>,
    #[serde(default)]
    pub sending_started: bool,
    #[serde(default)]
    pub response_status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_registration_cutoff: Option<u64>,
    pub request: u32,
    pub window: u64,
    pub publication: Option<u64>,
    pub projection: RequestProjection,
}

pub struct ProjectCandidate {
    publication: ProjectPublication,
    history: ProtocolHistory,
}

impl ProjectCandidate {
    pub fn changes_history(&self) -> bool {
        self.publication.item.is_some()
    }

    pub fn event(&self, active: &crate::ActiveTurn<'_>) -> crate::RecordEvent {
        let mut state = active.stored_state();
        state.committed_items = self.history.summary();
        crate::RecordEvent::ProjectPublished {
            state,
            publication: Box::new(self.publication.clone()),
            history: self.changes_history().then(|| self.history.archive()),
        }
    }
}

#[derive(Clone, Default)]
pub enum ProjectContextState {
    #[default]
    NotCollected,
    Loaded(Arc<ProjectSnapshot>),
    Failed {
        message: String,
        last_success: Option<Arc<ProjectSnapshot>>,
    },
}

impl crate::ActiveTurn<'_> {
    pub fn project_candidate(
        &self,
    ) -> Result<Option<ProjectCandidate>, crate::ConversationFailure> {
        if !self.view.settings.kind.is_user() {
            return Ok(None);
        }
        let ProjectContextState::Loaded(snapshot) = self.view.project_context.as_ref() else {
            return Err(crate::ConversationFailure::new("请求缺少成功的项目采集"));
        };
        let previous = self.view.project_publications.last();
        if previous.is_some_and(|previous| {
            previous.snapshot.hash == snapshot.hash
                && previous.snapshot.working_directory == snapshot.working_directory
                && previous.window == self.view.context.window
        }) {
            return Ok(None);
        }
        let id = previous
            .map_or(Some(1), |previous| previous.id.checked_add(1))
            .ok_or_else(|| crate::ConversationFailure::new("项目发布身份已耗尽"))?;
        let kind = if !snapshot.has_rules() {
            if previous.is_some_and(|publication| publication.snapshot.has_rules()) {
                agentelier_llm_model::PublicationKind::Removal
            } else {
                agentelier_llm_model::PublicationKind::Initial
            }
        } else if self
            .view
            .project_publications
            .iter()
            .any(|publication| publication.snapshot.has_rules())
        {
            agentelier_llm_model::PublicationKind::Replacement
        } else {
            agentelier_llm_model::PublicationKind::Initial
        };
        let kind = if previous.is_some_and(|previous| previous.window != self.view.context.window) {
            if snapshot.has_rules() {
                agentelier_llm_model::PublicationKind::Republished
            } else if self
                .view
                .project_publications
                .iter()
                .any(|publication| publication.snapshot.has_rules())
            {
                agentelier_llm_model::PublicationKind::RepublishedRemoval
            } else {
                agentelier_llm_model::PublicationKind::Initial
            }
        } else if previous.is_none() {
            agentelier_llm_model::PublicationKind::Initial
        } else {
            kind
        };
        let publication = ProjectPublication::new(
            id,
            self.view.context.window,
            snapshot.as_ref().clone(),
            kind,
        );
        publication
            .validate()
            .map_err(|failure| crate::ConversationFailure::new(failure.to_string()))?;
        let history = match &publication.item {
            Some(item) => self.history.with_project(item.clone()),
            None => self.history.clone(),
        };
        Ok(Some(ProjectCandidate {
            publication,
            history,
        }))
    }

    pub fn install_project(&mut self, candidate: ProjectCandidate) {
        *self.history = candidate.history;
        Arc::make_mut(&mut self.view.project_publications).push(Arc::new(candidate.publication));
        self.view.committed_items = self.history.summary();
        self.state.history_confirmed = true;
    }

    pub fn request_prepared(
        &mut self,
        projection: RequestProjection,
    ) -> Result<(), crate::ConversationFailure> {
        let turn = self
            .view
            .turn
            .as_mut()
            .ok_or_else(|| crate::ConversationFailure::new("请求记录缺少任务"))?;
        if projection.transcript.as_ref().map(|item| &item.reference)
            != turn.submission.transcript.as_ref()
        {
            return Err(crate::ConversationFailure::new(
                "请求与本轮选定文稿来源不一致",
            ));
        }
        turn.requests.push(PreparedRequestFact {
            response_accepted: Some(false),
            sending_started: false,
            response_status: None,
            task_registration_cutoff: Some(self.tasks.revision()),
            request: turn.request_index,
            window: self.view.context.window,
            publication: self
                .view
                .project_publications
                .last()
                .map(|publication| publication.id),
            projection,
        });
        Ok(())
    }

    pub fn loading_project(&mut self) {
        if let Some(turn) = &mut self.view.turn {
            turn.phase = crate::TaskPhase::LoadingProjectContext;
        }
    }

    pub fn project_context(&mut self, result: Result<ProjectSnapshot, String>) {
        *self.view.project_context = match result {
            Ok(snapshot) => ProjectContextState::Loaded(Arc::new(snapshot)),
            Err(message) => {
                let last_success = match self.view.project_context.as_ref() {
                    ProjectContextState::Loaded(snapshot) => Some(snapshot.clone()),
                    ProjectContextState::Failed { last_success, .. } => last_success.clone(),
                    ProjectContextState::NotCollected => None,
                };
                ProjectContextState::Failed {
                    message,
                    last_success,
                }
            }
        };
        if let Some(turn) = &mut self.view.turn {
            turn.phase = crate::TaskPhase::RequestingModel;
        }
    }
}
