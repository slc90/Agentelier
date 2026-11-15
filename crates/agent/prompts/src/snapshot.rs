use crate::library::{Library, PromptRef, PromptRevision};
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
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstructionSlot {
    Base,
    Supplement,
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
        snapshot.validate()?;
        Ok(snapshot)
    }
}

impl InstructionSnapshot {
    pub fn validate(&self) -> io::Result<()> {
        let mut bases = 0;
        for (order, instruction) in self.instructions.iter().enumerate() {
            instruction.revision.validate()?;
            if instruction.slot == InstructionSlot::Base {
                bases += 1;
                if order != 0 {
                    return Err(io::Error::other("基础指令必须位于首条"));
                }
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
