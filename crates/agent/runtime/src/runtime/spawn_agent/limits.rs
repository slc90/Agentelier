use super::Children;
use agentelier_conversation::{AgentId, AgentRegistry, ConversationFailure, SessionId};
use agentelier_tools::ToolResult;
use std::fmt;

pub(super) struct Limits {
    total: usize,
    concurrent: usize,
    depth: usize,
    pub(super) resident: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            total: 32,
            concurrent: 4,
            depth: 4,
            resident: 8,
        }
    }
}

impl Limits {
    pub(super) fn configured() -> Result<Self, ConversationFailure> {
        let defaults = Self::default();
        let limits = Self {
            total: Self::read("AGENTELIER_AGENT_TOTAL", defaults.total)?,
            concurrent: Self::read("AGENTELIER_AGENT_CONCURRENT", defaults.concurrent)?,
            depth: Self::read("AGENTELIER_AGENT_DEPTH", defaults.depth)?,
            resident: Self::read("AGENTELIER_AGENT_RESIDENT", defaults.resident)?,
        };
        tracing::debug!(
            total = limits.total,
            concurrent = limits.concurrent,
            depth = limits.depth,
            resident = limits.resident,
            "子 Agent 资源限制已配置"
        );
        Ok(limits)
    }

    fn read(variable: &str, default: usize) -> Result<usize, ConversationFailure> {
        match std::env::var(variable) {
            Ok(value) => value
                .parse()
                .map_err(|_| ConversationFailure::new(format!("{variable} 必须是非负整数"))),
            Err(std::env::VarError::NotPresent) => Ok(default),
            Err(error) => Err(ConversationFailure::new(format!(
                "无法读取 {variable}：{error}"
            ))),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) enum Capacity {
    Total(usize),
    Concurrent(usize),
    Depth(usize),
    Resident(usize),
}

impl Capacity {
    pub(super) fn rejection(self) -> ToolResult {
        let code = match self {
            Self::Total(_) => "agent_total_limit",
            Self::Concurrent(_) => "agent_concurrent_limit",
            Self::Depth(_) => "agent_depth_limit",
            Self::Resident(_) => "agent_residency_limit",
        };
        ToolResult::not_executed(code, &self.to_string())
    }
}

impl fmt::Display for Capacity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Total(limit) => write!(formatter, "会话已达到子 Agent 总量上限 {limit}"),
            Self::Concurrent(limit) => write!(formatter, "会话已达到子 Agent 执行并发上限 {limit}"),
            Self::Depth(limit) => write!(formatter, "子 Agent 派生将超过树深度上限 {limit}"),
            Self::Resident(limit) => {
                write!(formatter, "子 Agent 驻留上限 {limit} 已满且没有可卸载实例")
            }
        }
    }
}

impl Children {
    pub(in crate::runtime) fn with_configured_limits() -> Result<Self, ConversationFailure> {
        Ok(Self {
            limits: Limits::configured()?,
            ..Self::default()
        })
    }

    pub(super) fn check_spawn_capacity(
        &self,
        registry: &AgentRegistry,
        session: SessionId,
        parent: AgentId,
    ) -> Result<(), ToolResult> {
        let node = registry.get(session, parent).map_err(super::rejection)?;
        if usize::from(node.identity.depth) >= self.limits.depth {
            return Err(Capacity::Depth(self.limits.depth).rejection());
        }
        let children = registry
            .subtree(session, AgentId::for_root(session))
            .map_err(super::rejection)?
            .skip(1)
            .count();
        if children >= self.limits.total {
            return Err(Capacity::Total(self.limits.total).rejection());
        }
        self.check_execution_capacity().map_err(Capacity::rejection)
    }

    pub(super) fn check_execution_capacity(&self) -> Result<(), Capacity> {
        if self.active.len() >= self.limits.concurrent {
            return Err(Capacity::Concurrent(self.limits.concurrent));
        }
        Ok(())
    }
}
