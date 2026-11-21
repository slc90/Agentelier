use crate::{InstructionSlot, Library, PromptKind, PromptRef, PromptRevision, ResolvedInstruction};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::io;

const MAX_POLICY_BYTES: usize = 8 * 1024;
const MAX_ROLE_NAME_BYTES: usize = 64;
const ROOT_PROTOCOL: &str = "Use collaboration tools only when they are available. Delegate independent, well-scoped work, identify shared-file conflicts before assigning it, and keep dependent tasks ordered. Wait for actual results, verify claims and changes, reconcile conflicts, and give the user a single integrated answer.";
const SUB_AGENT_PROTOCOL: &str = "Carry out the work assigned by your parent Agent within its task and permission boundaries. Coordinate before changing shared files outside your assigned scope. Send your findings, changed files, and any unresolved work to your parent. Use an available collaboration tool to ask for clarification or more work. Keep internal coordination out of the user-facing reply.";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationMode {
    #[default]
    ExplicitRequestOnly,
    Proactive,
    Custom(String),
}

impl CollaborationMode {
    pub(crate) fn resolve(&self) -> io::Result<ResolvedInstruction> {
        let (mode, policy) = match self {
            Self::ExplicitRequestOnly => (
                "explicit_request_only",
                "Create child Agents only when the user explicitly asks for multiple Agents or parallel collaboration.",
            ),
            Self::Proactive => (
                "proactive",
                "Delegate independent parts of complex work when parallel execution materially helps.",
            ),
            Self::Custom(text) => {
                let text = text.trim();
                if text.is_empty()
                    || text.len() > MAX_POLICY_BYTES
                    || text.chars().any(|character| {
                        character.is_control() && !matches!(character, '\n' | '\r' | '\t')
                    })
                {
                    return Err(io::Error::other(
                        "自定义协作策略为空、含无效字符或超过 8 KiB",
                    ));
                }
                ("custom", text)
            }
        };
        Ok(ResolvedInstruction {
            slot: InstructionSlot::Collaboration,
            revision: revision(
                &format!("agentelier.collaboration.{mode}"),
                mode,
                format!("{ROOT_PROTOCOL}\n\n{policy}"),
            ),
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CustomRole {
    pub id: String,
    pub version: u32,
    pub instructions: PromptRef,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_tools: Option<BTreeSet<String>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct RoleSnapshot {
    pub id: String,
    pub version: u32,
    pub model: Option<String>,
    pub reasoning: Option<String>,
    allowed_tools: Option<BTreeSet<String>>,
    instructions: PromptRevision,
}

impl RoleSnapshot {
    pub fn narrow_tools(&self, parent: &BTreeSet<String>) -> io::Result<BTreeSet<String>> {
        let Some(allowed) = &self.allowed_tools else {
            return Ok(parent.clone());
        };
        if let Some(tool) = allowed.difference(parent).next() {
            return Err(io::Error::other(format!(
                "Agent Role {} 请求父级未授权的工具 {tool}",
                self.id
            )));
        }
        Ok(allowed.clone())
    }

    pub(crate) fn instructions(&self) -> [ResolvedInstruction; 2] {
        [
            ResolvedInstruction {
                slot: InstructionSlot::SubAgent,
                revision: revision(
                    "agentelier.collaboration.sub_agent",
                    "sub_agent",
                    SUB_AGENT_PROTOCOL.into(),
                ),
            },
            ResolvedInstruction {
                slot: InstructionSlot::Role,
                revision: self.instructions.clone(),
            },
        ]
    }
}

impl Library {
    pub fn resolve_role(&self, role: Option<&str>) -> io::Result<RoleSnapshot> {
        let id = role.unwrap_or("general");
        if let Some(text) = builtin_role(id) {
            return Ok(RoleSnapshot {
                id: id.into(),
                version: 1,
                model: None,
                reasoning: None,
                allowed_tools: None,
                instructions: revision(&format!("agentelier.role.{id}"), id, text.into()),
            });
        }
        let role = self
            .roles
            .iter()
            .find(|role| role.id == id)
            .ok_or_else(|| io::Error::other(format!("没有名为 {id} 的 Agent Role")))?;
        self.resolve_custom_role(role)
    }

    pub(crate) fn validate_roles(&self) -> io::Result<()> {
        self.collaboration.resolve()?;
        let mut seen = BTreeSet::new();
        for role in &self.roles {
            if !seen.insert(&role.id) {
                return Err(io::Error::other(format!("Agent Role {} 重复", role.id)));
            }
            self.resolve_custom_role(role)?;
        }
        Ok(())
    }

    fn resolve_custom_role(&self, role: &CustomRole) -> io::Result<RoleSnapshot> {
        let id = &role.id;
        let valid_setting = |value: &str| !value.trim().is_empty() && value.trim() == value;
        if id.is_empty()
            || id.len() > MAX_ROLE_NAME_BYTES
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
            || id == "root"
            || builtin_role(id).is_some()
            || role.version == 0
            || role
                .model
                .as_deref()
                .is_some_and(|model| !valid_setting(model))
            || role
                .reasoning
                .as_deref()
                .is_some_and(|effort| !valid_setting(effort))
            || role
                .allowed_tools
                .iter()
                .flatten()
                .any(|tool| !valid_setting(tool))
        {
            return Err(io::Error::other(format!(
                "Agent Role {id} 的标识、版本、模型或工具配置无效"
            )));
        }
        let instructions = self.find(&role.instructions).map_err(|failure| {
            io::Error::other(format!("Agent Role {id} 的指令引用无效：{failure}"))
        })?;
        instructions.validate()?;
        Ok(RoleSnapshot {
            id: id.clone(),
            version: role.version,
            model: role.model.clone(),
            reasoning: role.reasoning.clone(),
            allowed_tools: role.allowed_tools.clone(),
            instructions: instructions.clone(),
        })
    }
}

fn builtin_role(id: &str) -> Option<&'static str> {
    match id {
        "general" => {
            Some("Complete the assigned task using the evidence and capabilities available to you.")
        }
        "research" => Some(
            "Investigate the assigned question. Report evidence, sources, and uncertainty. Keep any changes within the assignment.",
        ),
        "implementation" => Some(
            "Implement the assigned change. Verify behavior through the normal entry points and report what you changed and verified.",
        ),
        "review" => Some(
            "Review the assigned work. Prioritize reproducible correctness and regression findings with concrete evidence. Leave changes to the responsible Agent unless your assignment explicitly includes fixes.",
        ),
        _ => None,
    }
}

fn revision(id: &str, name: &str, text: String) -> PromptRevision {
    PromptRevision {
        reference: PromptRef {
            id: id.into(),
            version: 1,
        },
        name: name.into(),
        language: "en".into(),
        pair: None,
        source: "Agentelier".into(),
        range: "全文".into(),
        dependencies: String::new(),
        kind: PromptKind::Complete,
        readonly: true,
        derived_from: None,
        hash: crate::content_hash(&text),
        text,
    }
}
