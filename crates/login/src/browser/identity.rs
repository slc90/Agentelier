use super::{AUTHORIZE_ENDPOINT, ISSUER, LoginFailure, LoginStage, TOKEN_ENDPOINT, parse_response};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};
use subtle::ConstantTimeEq;
use url::Url;

const CLOCK_TOLERANCE_SECONDS: u64 = 5;

#[derive(Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

#[derive(Deserialize)]
pub(crate) struct Claims {
    pub sub: String,
    pub name: Option<String>,
    pub email: Option<String>,
    pub(crate) nonce: Option<String>,
    pub(crate) auth_time: Option<u64>,
    iat: u64,
    exp: u64,
    pub(crate) aud: serde_json::Value,
    azp: Option<String>,
}

pub(crate) async fn verify(
    client: &reqwest::Client,
    id_token: &str,
    client_id: &str,
    nonce: Option<&str>,
) -> Result<Claims, LoginFailure> {
    let failure = || {
        LoginFailure::new(
            LoginStage::Identity,
            "ID Token 签名、身份或本次授权参数校验失败，原会话保持不变",
        )
    };
    let discovery: Discovery = get(
        client,
        &format!("{ISSUER}/.well-known/openid-configuration"),
    )
    .await?;
    let jwks = Url::parse(&discovery.jwks_uri).map_err(|_| failure())?;
    if discovery.issuer != ISSUER
        || discovery.authorization_endpoint != AUTHORIZE_ENDPOINT
        || discovery.token_endpoint != TOKEN_ENDPOINT
        || jwks.scheme() != "https"
        || jwks.host_str() != Some("auth.openai.com")
        || jwks.port_or_known_default() != Some(443)
        || !jwks.username().is_empty()
        || jwks.password().is_some()
        || jwks.fragment().is_some()
    {
        return Err(LoginFailure::new(
            LoginStage::Identity,
            "固定签发方的发现配置校验失败",
        ));
    }
    let keys: JwkSet = get(client, jwks.as_str()).await?;
    let header = decode_header(id_token).map_err(|_| failure())?;
    if header.alg != Algorithm::RS256 {
        return Err(failure());
    }
    let kid = header.kid.ok_or_else(failure)?;
    let key = DecodingKey::from_jwk(keys.find(&kid).ok_or_else(failure)?).map_err(|_| failure())?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["iss", "aud", "exp", "iat", "sub"]);
    validation.leeway = CLOCK_TOLERANCE_SECONDS;
    validation.validate_nbf = true;
    let claims = decode::<Claims>(id_token, &key, &validation)
        .map_err(|_| failure())?
        .claims;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| failure())?
        .as_secs();
    if claims.sub.trim().is_empty()
        || nonce.is_some_and(|expected| {
            claims
                .nonce
                .as_deref()
                .is_none_or(|actual| !bool::from(actual.as_bytes().ct_eq(expected.as_bytes())))
        })
        || claims.iat > now.saturating_add(CLOCK_TOLERANCE_SECONDS)
        || claims.exp <= claims.iat
        || claims
            .azp
            .as_ref()
            .is_some_and(|authorized| authorized != client_id)
        || claims.aud.as_array().is_some_and(|audiences| {
            audiences.len() > 1 && claims.azp.as_deref() != Some(client_id)
        })
    {
        return Err(failure());
    }
    Ok(claims)
}

async fn get<T: serde::de::DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
) -> Result<T, LoginFailure> {
    let response = client.get(url).send().await.map_err(|_| {
        LoginFailure::new(
            LoginStage::Identity,
            "无法获取签发方发现配置或身份验证公钥，请重试登录",
        )
    })?;
    parse_response(response, LoginStage::Identity).await
}
