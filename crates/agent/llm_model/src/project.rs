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
    #[serde(default)]
    pub kind: PublicationKind,
    pub id: u64,
    pub window: u64,
    pub snapshot: ProjectSnapshot,
    pub item: Option<ProjectItem>,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PublicationKind {
    #[default]
    Initial,
    Replacement,
    Removal,
    Republished,
    RepublishedRemoval,
}

impl ProjectPublication {
    pub fn initial(id: u64, window: u64, snapshot: ProjectSnapshot) -> Self {
        Self::new(id, window, snapshot, PublicationKind::Initial)
    }

    pub fn new(id: u64, window: u64, snapshot: ProjectSnapshot, kind: PublicationKind) -> Self {
        let text = match kind {
            PublicationKind::Initial => snapshot.rendered.clone(),
            PublicationKind::Replacement | PublicationKind::Republished => format!("The following complete project instructions replace all previously automatically published project instructions. User instructions and tool-provided material are unchanged.\n\n{}", snapshot.rendered),
            PublicationKind::Removal | PublicationKind::RepublishedRemoval => "All previously automatically published project instructions are withdrawn. No automatic project rules currently apply. User instructions and tool-provided material are unchanged.".into(),
        };
        let item = (!text.is_empty()).then(|| ProjectItem {
            publication: id,
            snapshot_hash: snapshot.hash.clone(),
            text,
        });
        Self {
            kind,
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
        let expected = Self::new(self.id, self.window, self.snapshot.clone(), self.kind);
        if self.id == 0
            || expected.item != self.item
            || (matches!(
                self.kind,
                PublicationKind::Removal | PublicationKind::RepublishedRemoval
            ) && self.snapshot.has_rules())
            || (matches!(
                self.kind,
                PublicationKind::Replacement | PublicationKind::Republished
            ) && !self.snapshot.has_rules())
        {
            return Err(crate::model::protocol("项目发布身份或正文不一致"));
        }
        Ok(())
    }
}
