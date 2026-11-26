use crate::{ServerId, Transport};
use agentelier_tools::{ToolDefinition, ToolSource};
use rmcp::{
    RoleClient,
    model::{PaginatedRequestParams, Tool},
    service::Peer,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub(crate) async fn list(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, &'static str> {
    tokio::time::timeout(crate::connections::STARTUP_TIMEOUT, pages(peer))
        .await
        .map_err(|_| "工具清单读取超时")?
}

async fn pages(peer: &Peer<RoleClient>) -> Result<Vec<Tool>, &'static str> {
    let mut tools = Vec::new();
    let mut cursor = None;
    let mut cursors = HashSet::new();
    loop {
        let result = peer
            .list_tools(Some(PaginatedRequestParams::default().with_cursor(cursor)))
            .await
            .map_err(|_| "服务器未能返回有效的 tools/list 结果")?;
        tools.extend(result.tools);
        let Some(next) = result.next_cursor else {
            return Ok(tools);
        };
        if !cursors.insert(next.clone()) {
            return Err("服务器返回重复的工具清单分页游标");
        }
        cursor = Some(next);
    }
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

pub(crate) fn convert(
    server: ServerId,
    label: &str,
    transport: &Transport,
    tools: Vec<Tool>,
) -> Catalog {
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
            normalize(server, label, &target, &tool)
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

fn normalize(
    server: ServerId,
    label: &str,
    target: &str,
    tool: &Tool,
) -> Result<ToolDefinition, &'static str> {
    let name = tool.name.as_ref();
    if name.is_empty() || name.len() > 256 || name.chars().any(char::is_control) {
        return Err("工具名称无效或过长");
    }
    let description = tool.description.as_deref().unwrap_or_default();
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
        mode: agentelier_tools::ToolMode::Asynchronous,
        name: format!("mcp_{readable}_{}", &digest[..32]),
        description: description.into(),
        parameters,
        source: ToolSource::Mcp {
            server,
            target: target.into(),
            tool: name.into(),
        },
        strict: false,
        output_schema: tool.output_schema.as_deref().cloned().map(Value::Object),
        display_source: Some(format!("MCP {label} · {name}")),
    })
}
