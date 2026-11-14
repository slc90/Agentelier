use super::{DYNAMIC_CLIENT, LoginFailure, LoginStage};
use base64::{Engine, engine::general_purpose::STANDARD};
use sha2::{Digest, Sha256};
use std::time::Duration;
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use url::Url;

const MAX_HEADER_BYTES: usize = 8192;
const MAX_HEADERS: usize = 32;
const CONNECTION_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_CLIENT_ID_LENGTH: usize = 200;
const CLEAR_QUERY: &str = "history.replaceState(null, '', '/auth/complete');";

pub(super) struct Callback {
    pub code: String,
    pub client_id: String,
}

pub(super) async fn receive(
    listener: &TcpListener,
    redirect: &str,
    state: &str,
    saved_client: Option<&str>,
) -> Result<Callback, LoginFailure> {
    loop {
        let (mut stream, peer) = listener
            .accept()
            .await
            .map_err(|error| LoginFailure::storage(LoginStage::Callback, error))?;
        if !peer.ip().is_loopback() {
            continue;
        }
        if let Ok(Some(result)) = tokio::time::timeout(
            CONNECTION_TIMEOUT,
            request(&mut stream, redirect, state, saved_client),
        )
        .await
        {
            return result;
        }
    }
}

async fn request(
    stream: &mut TcpStream,
    redirect: &str,
    state: &str,
    saved_client: Option<&str>,
) -> Option<Result<Callback, LoginFailure>> {
    let mut buffer = [0u8; MAX_HEADER_BYTES];
    let mut length = 0;
    let (path, host) = loop {
        if length == buffer.len() {
            respond(
                stream,
                "431 Request Header Fields Too Large",
                "Request too large",
                false,
            )
            .await;
            return None;
        }
        let count = stream.read(&mut buffer[length..]).await.ok()?;
        if count == 0 {
            return None;
        }
        length += count;
        let mut headers = [httparse::EMPTY_HEADER; MAX_HEADERS];
        let mut request = httparse::Request::new(&mut headers);
        match request.parse(&buffer[..length]) {
            Ok(httparse::Status::Partial) => continue,
            Ok(httparse::Status::Complete(_)) => {
                if request.method != Some("GET") {
                    respond(stream, "405 Method Not Allowed", "GET required", false).await;
                    return None;
                }
                let hosts: Vec<_> = request
                    .headers
                    .iter()
                    .filter(|header| header.name.eq_ignore_ascii_case("host"))
                    .collect();
                if hosts.len() != 1 {
                    respond(stream, "400 Bad Request", "Invalid host", false).await;
                    return None;
                }
                break (
                    request.path?.to_owned(),
                    std::str::from_utf8(hosts[0].value).ok()?.to_owned(),
                );
            }
            Err(_) => {
                respond(stream, "400 Bad Request", "Invalid request", false).await;
                return None;
            }
        }
    };
    let expected = Url::parse(redirect).ok()?;
    let expected_host = format!("127.0.0.1:{}", expected.port()?);
    if host != expected_host || !path.starts_with('/') || path.starts_with("//") {
        respond(stream, "400 Bad Request", "Invalid host", false).await;
        return None;
    }
    let url = expected.join(&path).ok()?;
    if url.origin() != expected.origin() || url.path() != "/auth/callback" {
        respond(stream, "404 Not Found", "Not found", false).await;
        return None;
    }
    let fields: Vec<_> = url.query_pairs().collect();
    let values = |key: &str| {
        fields
            .iter()
            .filter(|(name, _)| name == key)
            .map(|(_, value)| value.as_ref())
            .collect::<Vec<_>>()
    };
    let states = values("state");
    if states.len() != 1 || !bool::from(states[0].as_bytes().ct_eq(state.as_bytes())) {
        respond(
            stream,
            "400 Bad Request",
            "Invalid sign-in state. Return to the original browser tab.",
            false,
        )
        .await;
        return None;
    }
    let errors = values("error");
    let codes = values("code");
    let clients = values("client_id");
    if !errors.is_empty() {
        respond(
            stream,
            "200 OK",
            "Authorization was not completed. Return to Agentelier.",
            true,
        )
        .await;
        return Some(Err(LoginFailure::new(
            LoginStage::Callback,
            "用户或授权服务未完成授权，请重新登录",
        )));
    }
    let client = clients.first().copied().or(saved_client);
    let valid_client = client.filter(|id| {
        !id.is_empty()
            && id.len() <= MAX_CLIENT_ID_LENGTH
            && *id != DYNAMIC_CLIENT
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    });
    if codes.len() != 1
        || codes[0].is_empty()
        || clients.len() > 1
        || valid_client.is_none()
        || saved_client.is_some_and(|saved| client != Some(saved))
    {
        respond(
            stream,
            "400 Bad Request",
            "App registration was not completed. Return to Agentelier.",
            true,
        )
        .await;
        return Some(Err(LoginFailure::new(
            LoginStage::Callback,
            "授权码或实际客户端标识无效，原登记保持不变",
        )));
    }
    respond(
        stream,
        "200 OK",
        "Callback received. Return to Agentelier to view the result. You may close this tab.",
        true,
    )
    .await;
    Some(Ok(Callback {
        code: codes[0].to_owned(),
        client_id: valid_client?.to_owned(),
    }))
}

async fn respond(stream: &mut TcpStream, status: &str, message: &str, clear: bool) {
    let body = if clear {
        format!(
            "<!doctype html><meta charset=utf-8><title>Agentelier</title><h1>Return to Agentelier</h1><p>{message}</p><script>{CLEAR_QUERY}</script>"
        )
    } else {
        message.into()
    };
    let hash = STANDARD.encode(Sha256::digest(CLEAR_QUERY.as_bytes()));
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; script-src 'sha256-{hash}'; base-uri 'none'; frame-ancestors 'none'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // Browser disconnects do not invalidate an already received callback.
    if stream.write_all(response.as_bytes()).await.is_ok() {
        let _shutdown = stream.shutdown().await;
    }
}
