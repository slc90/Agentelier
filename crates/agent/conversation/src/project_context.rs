use agentelier_project_context::ProjectSnapshot;
use std::sync::Arc;

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

impl crate::ActiveTurn {
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
