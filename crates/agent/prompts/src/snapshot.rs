use crate::library::{Library, PromptRef, PromptRevision};
use crate::{CollaborationMode, RoleSnapshot};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io;

pub fn content_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub enum BaseInstruction {
    #[default]
    Default,
    Override(PromptRef),
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InstructionConfig {
    pub base: BaseInstruction,
    pub supplements: Vec<PromptRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collaboration: Option<CollaborationMode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionSlot {
    Base,
    Supplement,
    Collaboration,
    SubAgent,
    Role,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct ResolvedInstruction {
    pub slot: InstructionSlot,
    pub revision: PromptRevision,
}

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct InstructionSnapshot {
    pub base_overridden: bool,
    pub instructions: Vec<ResolvedInstruction>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct InstructionMessage {
    pub order: usize,
    pub role: String,
    pub slot: InstructionSlot,
    pub reference: PromptRef,
    pub hash: String,
    pub text: String,
}

impl Library {
    pub fn resolve(&self, config: &InstructionConfig) -> io::Result<InstructionSnapshot> {
        let mut snapshot = InstructionSnapshot::default();
        if let BaseInstruction::Override(reference) = &config.base {
            snapshot.base_overridden = true;
            snapshot.instructions.push(ResolvedInstruction {
                slot: InstructionSlot::Base,
                revision: self.find(reference)?.clone(),
            });
        }
        for reference in &config.supplements {
            snapshot.instructions.push(ResolvedInstruction {
                slot: InstructionSlot::Supplement,
                revision: self.find(reference)?.clone(),
            });
        }
        snapshot.instructions.push(
            config
                .collaboration
                .as_ref()
                .unwrap_or(&self.collaboration)
                .resolve()?,
        );
        snapshot.validate()?;
        Ok(snapshot)
    }
}

impl InstructionSnapshot {
    pub fn for_child(&self, role: &RoleSnapshot) -> io::Result<Self> {
        self.validate()?;
        let mut snapshot = Self {
            base_overridden: self.base_overridden,
            instructions: self
                .instructions
                .iter()
                .filter(|instruction| {
                    matches!(
                        instruction.slot,
                        InstructionSlot::Base | InstructionSlot::Supplement
                    )
                })
                .cloned()
                .collect(),
        };
        snapshot.instructions.extend(role.instructions());
        snapshot.validate()?;
        Ok(snapshot)
    }

    pub fn validate(&self) -> io::Result<()> {
        let mut bases = 0;
        let mut instructions = self.instructions.iter().enumerate().peekable();
        while let Some((order, instruction)) = instructions.next() {
            instruction.revision.validate()?;
            match instruction.slot {
                InstructionSlot::Base if order == 0 => bases += 1,
                InstructionSlot::Supplement => {}
                InstructionSlot::Collaboration if instructions.peek().is_none() => {}
                InstructionSlot::SubAgent => {
                    let Some((_, role)) = instructions
                        .next()
                        .filter(|(_, item)| item.slot == InstructionSlot::Role)
                    else {
                        return Err(io::Error::other("子 Agent 指令缺少角色来源"));
                    };
                    role.revision.validate()?;
                    if instructions.peek().is_some() {
                        return Err(io::Error::other("子 Agent 角色指令必须位于末尾"));
                    }
                }
                _ => return Err(io::Error::other("指令快照的来源或顺序不正确")),
            }
        }
        if bases != usize::from(self.base_overridden) {
            return Err(io::Error::other("基础指令条件与快照不一致"));
        }
        Ok(())
    }

    pub fn messages(&self) -> Vec<InstructionMessage> {
        self.instructions
            .iter()
            .enumerate()
            .map(|(order, instruction)| InstructionMessage {
                order,
                role: "developer".into(),
                slot: instruction.slot.clone(),
                reference: instruction.revision.reference.clone(),
                hash: instruction.revision.hash.clone(),
                text: instruction.revision.text.clone(),
            })
            .collect()
    }
}
