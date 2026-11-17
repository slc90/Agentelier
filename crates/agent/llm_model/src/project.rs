use agentelier_project_context::ProjectSnapshot;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectItem {
    pub publication: u64,
    pub snapshot_hash: String,
    pub text: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ProjectPublication {
    pub id: u64,
    pub window: u64,
    pub snapshot: ProjectSnapshot,
    pub item: Option<ProjectItem>,
}

impl ProjectPublication {
    pub fn initial(id: u64, window: u64, snapshot: ProjectSnapshot) -> Self {
        let item = snapshot.has_rules().then(|| ProjectItem {
            publication: id,
            snapshot_hash: snapshot.hash.clone(),
            text: snapshot.rendered.clone(),
        });
        Self {
            id,
            window,
            snapshot,
            item,
        }
    }

    pub fn validate(&self) -> Result<(), crate::ModelFailure> {
        self.snapshot
            .validate()
            .map_err(|message| crate::model::protocol(&message))?;
        let expected = Self::initial(self.id, self.window, self.snapshot.clone());
        if self.id == 0 || expected.item != self.item {
            return Err(crate::model::protocol("项目发布身份或正文不一致"));
        }
        Ok(())
    }
}
