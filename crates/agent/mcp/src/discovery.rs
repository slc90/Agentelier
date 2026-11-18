use crate::{ServerId, Transport};
use agentelier_tools::{ToolDefinition, ToolRegistration, ToolResult, ToolSource};
use rmcp::{
    RoleClient,
    model::{PaginatedRequestParams, Tool},
    service::Peer,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

const MAX_PAGES: usize = 16;
const MAX_TOOLS: usize = 128;
const MAX_LIST_BYTES: usize = 512 * 1024;
const MAX_SCHEMA_BYTES: usize = 32 * 1024;
const MAX_DESCRIPTION_BYTES: usize = 8 * 1024;
const LIST_DEADLINE: Duration = Duration::from_secs(12);

pub(crate) async fn list(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, &'static str> {
    tokio::time::timeout(LIST_DEADLINE, pages(peer))
        .await
        .map_err(|_| "工具清单读取超时")?
}

async fn pages(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, &'static str> {
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut bytes = 0usize;
    for _ in 0..MAX_PAGES {
        let result = peer
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .map_err(|_| "服务器未能返回有效的 tools/list 结果")?;
        bytes += serde_json::to_vec(&result)
            .map_err(|_| "工具清单无法序列化")?
            .len();
        if bytes > MAX_LIST_BYTES || tools.len() + result.tools.len() > MAX_TOOLS {
            return Err("工具清单超过数量或字节预算");
        }
        tools.extend(result.tools);
        let Some(next) = result.next_cursor else {
            return Ok(tools);
        };
        if !cursors.insert(next.clone()) {
            return Err("服务器返回重复的工具清单分页游标");
        }
        cursor = Some(next);
    }
    Err("工具清单超过分页预算")
}

pub(crate) fn transport_identity(transport: &Transport) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::json!(transport).to_string())
    )
}

pub(crate) struct Catalog {
    pub(crate) tools: Vec<ToolDefinition>,
    pub(crate) issues: Vec<String>,
}

pub(crate) fn convert(server: ServerId, transport: &Transport, tools: Vec<Tool>) -> Catalog {
    let target = transport_identity(transport);
    let mut names = HashMap::new();
    for tool in &tools {
        *names.entry(tool.name.clone()).or_insert(0usize) += 1;
    }
    let mut catalog = Catalog {
        tools: Vec::new(),
        issues: Vec::new(),
    };
    for tool in tools {
        let name = tool.name.as_ref();
        let entry = if names.get(name).copied().unwrap_or_default() > 1 {
            Err("原始名称重复")
        } else {
            normalize(server, &target, &tool)
        };
        match entry {
            Ok(definition) => catalog.tools.push(definition),
            Err(reason) => {
                let displayed: String = name
                    .chars()
                    .filter(|character| !character.is_control())
                    .take(64)
                    .collect();
                catalog.issues.push(format!("{displayed}：{reason}"));
            }
        }
    }
    catalog
}

fn normalize(server: ServerId, target: &str, tool: &Tool) -> Result<ToolDefinition, &'static str> {
    let name = tool.name.as_ref();
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("工具名称无效或过长");
    }
    let description = tool.description.as_deref().unwrap_or_default();
    if description.len() > MAX_DESCRIPTION_BYTES {
        return Err("工具描述过长");
    }
    let mut parameters = tool.input_schema.as_ref().clone();
    if parameters.get("type").and_then(Value::as_str) != Some("object") {
        return Err("输入 Schema 缺少 object 类型");
    }
    if !parameters
        .entry("properties")
        .or_insert_with(|| json!({}))
        .is_object()
    {
        return Err("输入 Schema 的 properties 必须为对象");
    }
    let parameters = Value::Object(parameters);
    if parameters.to_string().len() > MAX_SCHEMA_BYTES {
        return Err("输入 Schema 超过大小限制");
    }
    let mut digest = Sha256::new();
    digest.update(server.as_bytes());
    digest.update(name.as_bytes());
    let digest = format!("{:x}", digest.finalize());
    let readable: String = name
        .chars()
        .take(27)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect();
    Ok(ToolDefinition {
        name: format!("mcp_{readable}_{}", &digest[..32]),
        description: description.into(),
        parameters,
        source: ToolSource::Mcp {
            server,
            target: target.into(),
            tool: name.into(),
        },
        strict: false,
    })
}

pub(crate) fn register(definition: ToolDefinition) -> ToolRegistration {
    ToolRegistration::new(definition, |_, _, _| {
        Box::pin(async {
            ToolResult::not_executed(
                "mcp_discovery_only",
                "此版本已发现 MCP 工具，远端调用尚不可用",
            )
        })
    })
}
