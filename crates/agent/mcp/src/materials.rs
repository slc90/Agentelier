use crate::{ServerId, discovery};
use base64::Engine;
use rmcp::{
    RoleClient,
    model::{
        ContentBlock, GetPromptRequestParams, PaginatedRequestParams, ReadResourceRequestParams,
        ResourceContents, Role,
    },
    service::Peer,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    future::Future,
    time::Duration,
};
use tokio_util::sync::CancellationToken;

const MATERIAL_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_URI_BYTES: usize = 4096;
pub const MAX_MATERIAL_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub struct MaterialOrigin {
    pub server: ServerId,
    pub name: String,
    pub(crate) target: String,
}

#[derive(Clone, serde::Serialize)]
pub struct ResourceChoice {
    pub uri: String,
    pub name: String,
    pub description: Option<String>,
    pub mime: Option<String>,
    pub template: bool,
}

#[derive(Clone)]
pub struct MaterialPreview {
    pub text: String,
    pub usable: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum MaterialKind {
    #[default]
    Resources,
    Prompts,
}

#[derive(Clone, serde::Serialize)]
pub struct PromptChoice {
    pub name: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub parameters: Vec<PromptParameter>,
}

#[derive(Clone, serde::Serialize)]
pub struct PromptParameter {
    pub name: String,
    pub description: Option<String>,
    pub required: bool,
}

pub enum MaterialRequest {
    List {
        server: ServerId,
        kind: MaterialKind,
    },
    GetPrompt {
        origin: MaterialOrigin,
        name: String,
        arguments: BTreeMap<String, String>,
    },
    ReadResource {
        origin: MaterialOrigin,
        uri: String,
    },
}

#[derive(Clone)]
pub enum MaterialReply {
    Prompts {
        origin: MaterialOrigin,
        entries: Vec<PromptChoice>,
    },
    Resources {
        origin: MaterialOrigin,
        entries: Vec<ResourceChoice>,
    },
    Preview(MaterialPreview),
}

impl MaterialRequest {
    pub(crate) fn server(&self) -> ServerId {
        match self {
            Self::List { server, .. } => *server,
            Self::ReadResource { origin, .. } | Self::GetPrompt { origin, .. } => origin.server,
        }
    }

    pub(crate) fn kind(&self) -> MaterialKind {
        match self {
            Self::List { kind, .. } => *kind,
            Self::ReadResource { .. } => MaterialKind::Resources,
            Self::GetPrompt { .. } => MaterialKind::Prompts,
        }
    }

    pub(crate) fn matches(&self, transport: &crate::Transport) -> bool {
        match self {
            Self::List { .. } => true,
            Self::ReadResource { origin, .. } | Self::GetPrompt { origin, .. } => {
                origin.target == discovery::transport_identity(transport)
            }
        }
    }
}

pub(crate) async fn execute(
    peer: Peer<RoleClient>,
    origin: MaterialOrigin,
    request: MaterialRequest,
    closed: CancellationToken,
    cancel: CancellationToken,
) -> Result<MaterialReply, String> {
    let operation = async {
        match request {
            MaterialRequest::List {
                kind: MaterialKind::Resources,
                ..
            } => load_resources(&peer, origin).await,
            MaterialRequest::List {
                kind: MaterialKind::Prompts,
                ..
            } => load_prompts(&peer, origin).await,
            MaterialRequest::GetPrompt {
                name, arguments, ..
            } => get_prompt(&peer, &origin, name, arguments).await,
            MaterialRequest::ReadResource { uri, .. } => read_resource(&peer, &origin, &uri).await,
        }
    };
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err("已取消 MCP 材料操作".into()),
        _ = closed.cancelled() => Err("原 MCP 连接已关闭".into()),
        result = tokio::time::timeout(MATERIAL_TIMEOUT, operation) =>
            result.map_err(|_| "MCP 材料操作超时".to_owned())?,
    }
}

async fn load_prompts(
    peer: &Peer<RoleClient>,
    origin: MaterialOrigin,
) -> Result<MaterialReply, String> {
    let entries = pages(|params| async move {
        let reply = peer
            .list_prompts(Some(params))
            .await
            .map_err(|_| "无法读取 MCP 提示词清单")?;
        let entries = reply
            .prompts
            .into_iter()
            .map(|prompt| PromptChoice {
                name: prompt.name,
                title: prompt.title,
                description: prompt.description,
                parameters: prompt
                    .arguments
                    .unwrap_or_default()
                    .into_iter()
                    .map(|arg| PromptParameter {
                        name: arg.name,
                        description: arg.description,
                        required: arg.required.unwrap_or(false),
                    })
                    .collect(),
            })
            .collect();
        Ok((entries, reply.next_cursor))
    })
    .await?;
    Ok(MaterialReply::Prompts { origin, entries })
}

async fn get_prompt(
    peer: &Peer<RoleClient>,
    origin: &MaterialOrigin,
    name: String,
    arguments: BTreeMap<String, String>,
) -> Result<MaterialReply, String> {
    let encoded = serde_json::to_vec(&arguments).map_err(|_| "提示词参数无法编码")?;
    if name.trim().is_empty() || encoded.len() > MAX_MATERIAL_BYTES {
        return Err("MCP 提示词名称为空或参数超过预算".into());
    }
    let argument_names = arguments
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    let params = GetPromptRequestParams::new(name.clone()).with_arguments(
        arguments
            .into_iter()
            .map(|(name, value)| (name, serde_json::Value::String(value)))
            .collect(),
    );
    let result = peer.get_prompt(params).await.map_err(|error| match error {
        rmcp::service::ServiceError::McpError(error) => format!(
            "服务器拒绝展开 MCP 提示词，JSON-RPC 错误码 {}",
            error.code.0
        ),
        _ => "无法取得有效的 MCP 提示词展开结果".into(),
    })?;
    let usable = matches!(result.messages.as_slice(), [message] if message.role == Role::User && matches!(&message.content, ContentBlock::Text(text) if !text.text.is_empty()));
    let mut text = format!(
        "[MCP Prompt]\nServer: {} ({})\nTemplate: {name}\nArguments: {argument_names}\n",
        origin.name, origin.server
    );
    if let Some(description) = result.description {
        text.push_str(&format!("Description: {description}\n"));
    }
    if !usable {
        text.push_str("只有单条非空的 user 纯文本消息可加入聊天草稿。\n");
    }
    for message in result.messages {
        text.push_str(&format!("\nRole: {:?}\n", message.role));
        match message.content {
            ContentBlock::Text(body) => {
                text.push_str(&format!(
                    "UTF-8 bytes: {}\nSHA-256: {:x}\n\n{}\n",
                    body.text.len(),
                    Sha256::digest(body.text.as_bytes()),
                    body.text
                ));
            }
            ContentBlock::ResourceLink(resource) => {
                text.push_str(&format!("resource_link · {}\n", resource.uri))
            }
            ContentBlock::Resource(resource) => match resource.resource {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text: body,
                    ..
                } => text.push_str(&format!(
                    "text resource · {uri} · {mime_type:?} · {} bytes\n",
                    body.len()
                )),
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => text.push_str(&format!(
                    "binary resource · {uri} · {mime_type:?} · {} Base64 bytes\n",
                    blob.len()
                )),
                _ => text.push_str("embedded resource\n"),
            },
            ContentBlock::Image(image) => text.push_str(&format!(
                "image · {} · {} Base64 bytes\n",
                image.mime_type,
                image.data.len()
            )),
            ContentBlock::Audio(audio) => text.push_str(&format!(
                "audio · {} · {} Base64 bytes\n",
                audio.mime_type,
                audio.data.len()
            )),
            _ => text.push_str("不支持的内容块\n"),
        }
    }
    let text = bounded_preview(text, "\n[/MCP Prompt]");
    Ok(MaterialReply::Preview(MaterialPreview { text, usable }))
}

async fn load_resources(
    peer: &Peer<RoleClient>,
    origin: MaterialOrigin,
) -> Result<MaterialReply, String> {
    let mut entries = pages(|params| async move {
        let reply = peer
            .list_resources(Some(params))
            .await
            .map_err(|_| "无法读取 MCP 资源清单")?;
        let entries = reply
            .resources
            .into_iter()
            .map(|item| ResourceChoice {
                uri: item.uri,
                name: item.name,
                description: item.description,
                mime: item.mime_type,
                template: false,
            })
            .collect();
        Ok((entries, reply.next_cursor))
    })
    .await?;
    let templates = pages(|params| async move {
        let reply = match peer.list_resource_templates(Some(params)).await {
            Ok(reply) => reply,
            Err(rmcp::service::ServiceError::McpError(error)) if error.code.0 == -32601 => {
                return Ok((Vec::new(), None));
            }
            Err(_) => return Err("无法读取 MCP 资源模板清单"),
        };
        let entries = reply
            .resource_templates
            .into_iter()
            .map(|item| ResourceChoice {
                uri: item.uri_template,
                name: item.name,
                description: item.description,
                mime: item.mime_type,
                template: true,
            })
            .collect();
        Ok((entries, reply.next_cursor))
    })
    .await?;
    entries.extend(templates);
    Ok(MaterialReply::Resources { origin, entries })
}

pub(crate) async fn pages<T, F, Fut>(mut fetch: F) -> Result<Vec<T>, String>
where
    F: FnMut(PaginatedRequestParams) -> Fut,
    Fut: Future<Output = Result<(Vec<T>, Option<String>), &'static str>>,
{
    let mut cursor = None;
    let mut cursors = HashSet::new();
    let mut items = Vec::new();
    loop {
        let (page, next) = fetch(PaginatedRequestParams::default().with_cursor(cursor)).await?;
        items.extend(page);
        let Some(next) = next else { return Ok(items) };
        if !cursors.insert(next.clone()) {
            return Err("MCP 服务器重复使用清单分页游标".into());
        }
        cursor = Some(next);
    }
}

pub fn expand_resource_uri(
    template: &str,
    values: &BTreeMap<String, String>,
) -> Result<String, String> {
    if template.len() > MAX_URI_BYTES || values.values().any(|value| value.len() > MAX_URI_BYTES) {
        return Err("URI 模板或参数过长".into());
    }
    for expression in template.split('{').skip(1) {
        let (expression, _) = expression.split_once('}').ok_or("URI 模板表达式未闭合")?;
        for spec in expression
            .trim_start_matches(['+', '#', '.', '/', ';', '?', '&'])
            .split(',')
        {
            let name = spec
                .split(':')
                .next()
                .unwrap_or_default()
                .trim_end_matches('*');
            if !values.contains_key(name) {
                return Err(format!("缺少 URI 模板参数 {name}"));
            }
        }
    }
    let mut expanded = uritemplate::UriTemplate::new(template);
    for (name, value) in values {
        expanded.set(name.as_str(), value.as_str());
    }
    let uri = expanded.build();
    check_uri(&uri)?;
    Ok(uri)
}

fn check_uri(uri: &str) -> Result<(), String> {
    if uri.len() > MAX_URI_BYTES
        || uri.chars().any(|c| c.is_control() || c.is_whitespace())
        || url::Url::parse(uri).is_err()
    {
        return Err("请输入有效、长度受限的绝对资源 URI".into());
    }
    Ok(())
}

async fn read_resource(
    peer: &Peer<RoleClient>,
    origin: &MaterialOrigin,
    uri: &str,
) -> Result<MaterialReply, String> {
    check_uri(uri)?;
    let reply = peer
        .read_resource(ReadResourceRequestParams::new(uri))
        .await
        .map_err(|error| match error {
            rmcp::service::ServiceError::McpError(error) => {
                format!("MCP 服务器拒绝资源读取，JSON-RPC 错误码 {}", error.code.0)
            }
            _ => "无法取得有效的 MCP 资源读取结果".into(),
        })?;
    if reply.contents.is_empty() {
        return Err("资源没有返回内容".into());
    }
    let mut usable = true;
    let mut text = format!(
        "[MCP Resource]\nServer: {} ({})\nRequested URI: {uri}\n",
        origin.name, origin.server
    );
    for content in reply.contents {
        match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text: body,
                ..
            } => {
                let mime = mime_type.as_deref().unwrap_or("text/plain");
                let media = mime
                    .split(';')
                    .next()
                    .unwrap_or(mime)
                    .trim()
                    .to_ascii_lowercase();
                let text_supported = media.starts_with("text/")
                    || matches!(media.as_str(), "application/json" | "application/xml")
                    || media.ends_with("+json")
                    || media.ends_with("+xml");
                if text_supported {
                    text.push_str(&format!(
                        "\nURI: {uri}\nMIME: {mime}\nUTF-8 bytes: {}\nSHA-256: {:x}\n\n{body}\n",
                        body.len(),
                        Sha256::digest(body.as_bytes())
                    ));
                } else {
                    usable = false;
                    text.push_str(&format!(
                        "\nURI: {uri}\nMIME: {mime}\n{} bytes\n此 MIME 暂无法加入聊天。\n",
                        body.len()
                    ));
                }
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                usable = false;
                let size = base64::engine::general_purpose::STANDARD
                    .decode(blob)
                    .map_err(|_| "服务器返回的资源 blob 编码无效")?
                    .len();
                let mime = mime_type.as_deref().unwrap_or("application/octet-stream");
                text.push_str(&format!(
                    "\nURI: {uri}\nMIME: {mime}\n{size} bytes\n二进制资源暂无法加入聊天。\n"
                ));
            }
            _ => {
                usable = false;
                text.push_str("\n服务器返回了暂不支持的资源内容。\n");
            }
        }
    }
    let text = bounded_preview(text, "\n[/MCP Resource]");
    Ok(MaterialReply::Preview(MaterialPreview { text, usable }))
}

fn bounded_preview(mut text: String, closing: &str) -> String {
    const TRUNCATED: &str = "\n[预览已截断]";
    if text.len() > MAX_MATERIAL_BYTES - closing.len() {
        let prefix = MAX_MATERIAL_BYTES - closing.len() - TRUNCATED.len();
        text.truncate(text.floor_char_boundary(prefix));
        text.push_str(TRUNCATED);
    }
    text.push_str(closing);
    text
}
