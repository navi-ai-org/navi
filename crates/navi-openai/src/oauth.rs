use navi_core::{CommandCodeCredentialMetadata, CredentialStore, XAI_GROK_CLI_OAUTH_KIND};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

#[derive(Debug)]
pub struct DeviceOAuthStarted {
    pub verification_uri: String,
    pub user_code: String,
    /// Optional slot the TUI can write an authorization code into when the
    /// browser shows "copy this code" instead of redirecting to loopback.
    pub paste_slot: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
}

const COMMANDCODE_DEFAULT_API_BASE: &str = "https://api.commandcode.ai";
const COMMANDCODE_DEFAULT_STUDIO_BASE: &str = "https://commandcode.ai";
const COMMANDCODE_CLI_VERSION: &str = "0.38.2";
const OPENAI_DEFAULT_ISSUER: &str = "https://auth.openai.com";
const OPENAI_DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_CALLBACK_PATH: &str = "/auth/callback";

/// xAI Grok CLI public OIDC client (same as official `grok` binary).
const XAI_DEFAULT_ISSUER: &str = "https://auth.x.ai";
const XAI_DEFAULT_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_CALLBACK_PATH: &str = "/callback";
const XAI_DEFAULT_SCOPES: &str =
    "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write";
/// Base URL for Grok CLI session tokens (OAuth), not Platform API keys.
pub const XAI_GROK_CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
/// Early refresh buffer: refresh when fewer than this many seconds remain.
const XAI_REFRESH_SKEW_SECS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCodeUsageData {
    pub whoami: Value,
    pub credits: Option<Value>,
    pub subscription: Option<Value>,
    pub usage_summary: Option<Value>,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiUsageReport {
    pub plan_type: Option<String>,
    pub limit_reached_kind: Option<String>,
    pub limits: Vec<OpenAiUsageLimitSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiUsageLimitSnapshot {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub metered_feature: Option<String>,
    pub limit_reached: bool,
    pub primary: Option<OpenAiUsageWindow>,
    pub secondary: Option<OpenAiUsageWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenAiUsageWindow {
    pub used_percent: i32,
    pub limit_window_seconds: i32,
    pub reset_after_seconds: i32,
    pub reset_at: i32,
}

pub async fn github_copilot_device_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    mut on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    const CLIENT_ID: &str = "Ov23li8tweQw6odWQebz";
    let client = reqwest::Client::new();
    let device_response = client
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .json(&serde_json::json!({
            "client_id": CLIENT_ID,
            "scope": "read:user",
        }))
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !device_response.status().is_success() {
        return Err(format!(
            "device authorization failed: {}",
            device_response.status()
        ));
    }

    let device_data: serde_json::Value = device_response
        .json()
        .await
        .map_err(|err| err.to_string())?;
    let verification_uri = device_data
        .get("verification_uri")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing verification URL".to_string())?
        .to_string();
    let user_code = device_data
        .get("user_code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing user code".to_string())?
        .to_string();
    let device_code = device_data
        .get("device_code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing device code".to_string())?
        .to_string();
    let mut interval = device_data
        .get("interval")
        .and_then(|value| value.as_u64())
        .unwrap_or(5)
        .max(1);

    on_started(DeviceOAuthStarted {
        verification_uri,
        user_code,
        paste_slot: None,
    });

    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(interval + 3)).await;
        let token_response = client
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .header("User-Agent", "navi/0.1.0")
            .json(&serde_json::json!({
                "client_id": CLIENT_ID,
                "device_code": device_code,
                "grant_type": "urn:ietf:params:oauth:grant-type:device_code",
            }))
            .send()
            .await
            .map_err(|err| err.to_string())?;

        if !token_response.status().is_success() {
            return Err(format!(
                "token exchange failed: {}",
                token_response.status()
            ));
        }

        let token_data: serde_json::Value =
            token_response.json().await.map_err(|err| err.to_string())?;
        if let Some(access_token) = token_data
            .get("access_token")
            .and_then(|value| value.as_str())
        {
            credential_store
                .set_api_key(provider_id, access_token)
                .map_err(|err| err.to_string())?;
            return Ok(());
        }

        match token_data.get("error").and_then(|value| value.as_str()) {
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some(error) => return Err(error.to_string()),
            None => {}
        }
    }

    Err("device authorization timed out".to_string())
}

pub async fn openai_usage_report(
    access_token: &str,
) -> std::result::Result<OpenAiUsageReport, String> {
    let response = reqwest::Client::new()
        .get(openai_usage_url())
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI usage request failed: {status}: {body}"));
    }

    let payload = response
        .json::<OpenAiUsagePayload>()
        .await
        .map_err(|err| err.to_string())?;
    Ok(payload.into_report())
}

/// OpenRouter account usage from `GET /api/v1/key`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenRouterUsageReport {
    pub label: Option<String>,
    pub is_free_tier: Option<bool>,
    pub usage: Option<f64>,
    pub usage_daily: Option<f64>,
    pub usage_weekly: Option<f64>,
    pub usage_monthly: Option<f64>,
    pub limit: Option<f64>,
    pub limit_remaining: Option<f64>,
    pub limit_reset: Option<String>,
}

pub async fn openrouter_usage_report(
    api_key: &str,
) -> std::result::Result<OpenRouterUsageReport, String> {
    let response = reqwest::Client::new()
        .get("https://openrouter.ai/api/v1/key")
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenRouter usage request failed: {status}: {body}"));
    }
    let payload: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let data = payload.get("data").cloned().unwrap_or(payload);
    Ok(OpenRouterUsageReport {
        label: data
            .get("label")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        is_free_tier: data.get("is_free_tier").and_then(|v| v.as_bool()),
        usage: data.get("usage").and_then(|v| v.as_f64()),
        usage_daily: data.get("usage_daily").and_then(|v| v.as_f64()),
        usage_weekly: data.get("usage_weekly").and_then(|v| v.as_f64()),
        usage_monthly: data.get("usage_monthly").and_then(|v| v.as_f64()),
        limit: data.get("limit").and_then(|v| v.as_f64()),
        limit_remaining: data.get("limit_remaining").and_then(|v| v.as_f64()),
        limit_reset: data
            .get("limit_reset")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

/// xAI / Grok account usage from the CLI billing proxy (OAuth session tokens).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XaiUsageReport {
    pub credit_usage_percent: Option<f64>,
    pub period_type: Option<String>,
    pub period_start: Option<String>,
    pub period_end: Option<String>,
    pub product_usage: Vec<XaiProductUsage>,
    pub prepaid_balance: Option<f64>,
    pub on_demand_used: Option<f64>,
    pub on_demand_cap: Option<f64>,
    pub is_unified_billing: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct XaiProductUsage {
    pub product: String,
    pub usage_percent: f64,
}

pub async fn xai_usage_report(access_token: &str) -> std::result::Result<XaiUsageReport, String> {
    // Billing lives on the Grok CLI chat proxy and requires the CLI token
    // auth header + client version (otherwise 426 / 401).
    let response = reqwest::Client::new()
        .get(format!(
            "{}/billing?format=credits",
            XAI_GROK_CLI_BASE_URL.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .header("X-XAI-Token-Auth", "xai-grok-cli")
        .header("x-grok-client-version", "0.2.91")
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("xAI usage request failed: {status}: {body}"));
    }
    let payload: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    let config = payload.get("config").cloned().unwrap_or(payload);
    let period = config.get("currentPeriod");
    let product_usage = config
        .get("productUsage")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    Some(XaiProductUsage {
                        product: item.get("product")?.as_str()?.to_string(),
                        usage_percent: item.get("usagePercent")?.as_f64().unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(XaiUsageReport {
        credit_usage_percent: config.get("creditUsagePercent").and_then(|v| v.as_f64()),
        period_type: period
            .and_then(|p| p.get("type"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        period_start: period
            .and_then(|p| p.get("start"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        period_end: period
            .and_then(|p| p.get("end"))
            .and_then(|v| v.as_str())
            .map(str::to_string),
        product_usage,
        prepaid_balance: config
            .get("prepaidBalance")
            .and_then(|v| v.get("val"))
            .and_then(|v| v.as_f64()),
        on_demand_used: config
            .get("onDemandUsed")
            .and_then(|v| v.get("val"))
            .and_then(|v| v.as_f64()),
        on_demand_cap: config
            .get("onDemandCap")
            .and_then(|v| v.get("val"))
            .and_then(|v| v.as_f64()),
        is_unified_billing: config.get("isUnifiedBillingUser").and_then(|v| v.as_bool()),
    })
}

pub async fn openai_browser_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    mut on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    let (port, listener) = openai_auth_listener()?;
    let redirect_uri = format!("http://localhost:{port}{OPENAI_CALLBACK_PATH}");
    let state = generate_oauth_token();
    let pkce = PkceCodes::generate();
    let issuer = openai_issuer();
    let client_id = openai_client_id();
    let auth_url = openai_authorize_url(&issuer, &client_id, &redirect_uri, &pkce, &state);

    on_started(DeviceOAuthStarted { verification_uri: auth_url, user_code: String::new(), paste_slot: None });

    let code = tokio::task::spawn_blocking(move || {
        wait_for_openai_callback(listener, &state, Duration::from_secs(300))
    })
    .await
    .map_err(|err| err.to_string())??;

    let tokens =
        exchange_openai_code_for_tokens(&issuer, &client_id, &redirect_uri, &pkce, &code).await?;
    credential_store
        .set_oauth_credential(provider_id, &tokens.access_token, "chatgpt-codex")
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Browser OIDC login for xAI Grok (Authorization Code + PKCE, loopback redirect).
pub async fn xai_browser_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    mut on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    let (port, listener) = xai_auth_listener()?;
    let redirect_uri = format!("http://127.0.0.1:{port}{XAI_CALLBACK_PATH}");
    let state = generate_oauth_token();
    let pkce = PkceCodes::generate();
    let issuer = xai_issuer();
    let client_id = xai_client_id();
    let auth_url = xai_authorize_url(&issuer, &client_id, &redirect_uri, &pkce, &state);

    let paste_slot = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
    on_started(DeviceOAuthStarted {
        verification_uri: auth_url,
        user_code: String::new(),
        paste_slot: Some(paste_slot.clone()),
    });

    let code = tokio::task::spawn_blocking(move || {
        wait_for_xai_callback(listener, &state, paste_slot, Duration::from_secs(600))
    })
    .await
    .map_err(|err| err.to_string())??;

    let tokens =
        exchange_xai_code_for_tokens(&issuer, &client_id, &redirect_uri, &pkce, &code).await?;
    store_xai_tokens(&credential_store, provider_id, &tokens)?;
    Ok(())
}

/// Device-code OIDC login for xAI Grok (headless / remote environments).
pub async fn xai_device_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    mut on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    let issuer = xai_issuer();
    let client_id = xai_client_id();
    let client = reqwest::Client::new();

    let device_body = [
        ("client_id", client_id.as_str()),
        ("scope", XAI_DEFAULT_SCOPES),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");

    let device_response = client
        .post(format!(
            "{}/oauth2/device/code",
            issuer.trim_end_matches('/')
        ))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(device_body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if device_response.status().as_u16() == 404 {
        return Err(
            "xAI device-code endpoint unavailable (404); use browser OAuth instead".to_string(),
        );
    }
    if !device_response.status().is_success() {
        let status = device_response.status();
        let body = device_response.text().await.unwrap_or_default();
        return Err(format!("xAI device authorization failed: {status}: {body}"));
    }

    let device_data: serde_json::Value = device_response
        .json()
        .await
        .map_err(|err| err.to_string())?;
    let verification_uri = device_data
        .get("verification_uri_complete")
        .or_else(|| device_data.get("verification_uri"))
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing verification URL".to_string())?
        .to_string();
    let user_code = device_data
        .get("user_code")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let device_code = device_data
        .get("device_code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "missing device code".to_string())?
        .to_string();
    let mut interval = device_data
        .get("interval")
        .and_then(|value| value.as_u64())
        .unwrap_or(5)
        .max(1);

    on_started(DeviceOAuthStarted {
        verification_uri,
        user_code,
        paste_slot: None,
    });

    for _ in 0..120 {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let token_body = [
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ("device_code", device_code.as_str()),
            ("client_id", client_id.as_str()),
        ]
        .into_iter()
        .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
        .collect::<Vec<_>>()
        .join("&");

        let token_response = client
            .post(format!("{}/oauth2/token", issuer.trim_end_matches('/')))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(token_body)
            .send()
            .await
            .map_err(|err| err.to_string())?;

        if !token_response.status().is_success() {
            let status = token_response.status();
            let body = token_response.text().await.unwrap_or_default();
            if let Ok(err_json) = serde_json::from_str::<serde_json::Value>(&body) {
                match err_json.get("error").and_then(|v| v.as_str()) {
                    Some("authorization_pending") => continue,
                    Some("slow_down") => {
                        interval += 5;
                        continue;
                    }
                    Some(error) => return Err(error.to_string()),
                    None => {}
                }
            }
            return Err(format!("xAI token exchange failed: {status}: {body}"));
        }

        let tokens: XaiTokenResponse = token_response.json().await.map_err(|err| err.to_string())?;
        if tokens.access_token.is_empty() {
            continue;
        }
        store_xai_tokens(&credential_store, provider_id, &tokens)?;
        return Ok(());
    }

    Err("xAI device authorization timed out".to_string())
}

/// Default xAI OAuth entry point used by the TUI: browser loopback PKCE.
pub async fn xai_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    if std::env::var("NAVI_XAI_OAUTH_DEVICE")
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return xai_device_oauth(credential_store, provider_id, on_started).await;
    }
    xai_browser_oauth(credential_store, provider_id, on_started).await
}

/// Refresh a stored xAI access token when it is near expiry.
pub async fn ensure_xai_access_token(
    credential_store: &CredentialStore,
    provider_id: &str,
) -> std::result::Result<Option<String>, String> {
    let Some(kind) = credential_store.get_oauth_api_kind(provider_id) else {
        return Ok(credential_store.get_model_api_key(provider_id));
    };
    if kind != XAI_GROK_CLI_OAUTH_KIND {
        return Ok(credential_store.get_model_api_key(provider_id));
    }

    let Some(access) = credential_store.get_api_key(provider_id) else {
        return Ok(None);
    };

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expires_at = credential_store
        .get_oauth_expires_at(provider_id)
        .unwrap_or(i64::MAX);
    if expires_at - XAI_REFRESH_SKEW_SECS > now {
        return Ok(Some(access));
    }

    let Some(refresh_token) = credential_store.get_oauth_refresh_token(provider_id) else {
        return Ok(Some(access));
    };

    let issuer = xai_issuer();
    let client_id = xai_client_id();
    let body = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token.as_str()),
        ("client_id", client_id.as_str()),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");

    let response = reqwest::Client::new()
        .post(format!("{}/oauth2/token", issuer.trim_end_matches('/')))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("xAI token refresh failed: {status}: {body}"));
    }

    let mut tokens: XaiTokenResponse = response.json().await.map_err(|err| err.to_string())?;
    if tokens.refresh_token.is_none() {
        tokens.refresh_token = Some(refresh_token);
    }
    store_xai_tokens(credential_store, provider_id, &tokens)?;
    Ok(Some(tokens.access_token))
}

/// Returns true when `token` looks like an xAI OAuth access JWT (not a Platform API key).
pub fn is_xai_oauth_access_token(token: &str) -> bool {
    let token = token.trim();
    !token.is_empty()
        && !token.starts_with("xai-")
        && token.starts_with("eyJ")
        && token.matches('.').count() >= 2
}

fn store_xai_tokens(
    credential_store: &CredentialStore,
    provider_id: &str,
    tokens: &XaiTokenResponse,
) -> std::result::Result<(), String> {
    let expires_at = tokens.expires_in.map(|secs| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        now + secs as i64
    });
    credential_store
        .set_oauth_credential_full(
            provider_id,
            &tokens.access_token,
            XAI_GROK_CLI_OAUTH_KIND,
            tokens.refresh_token.as_deref(),
            expires_at,
        )
        .map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
struct XaiTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

async fn exchange_xai_code_for_tokens(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> std::result::Result<XaiTokenResponse, String> {
    let body = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", &pkce.code_verifier),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");

    let response = reqwest::Client::new()
        .post(format!("{}/oauth2/token", issuer.trim_end_matches('/')))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("xAI token exchange failed: {status}: {body}"));
    }

    response.json().await.map_err(|err| err.to_string())
}

fn xai_issuer() -> String {
    std::env::var("NAVI_XAI_OAUTH_ISSUER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| XAI_DEFAULT_ISSUER.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn xai_client_id() -> String {
    std::env::var("NAVI_XAI_OAUTH_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| XAI_DEFAULT_CLIENT_ID.to_string())
}

fn xai_auth_listener() -> std::result::Result<(u16, TcpListener), String> {
    for port in 8765..8785 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                listener
                    .set_nonblocking(true)
                    .map_err(|err| err.to_string())?;
                return Ok((port, listener));
            }
            Err(_) => continue,
        }
    }
    match TcpListener::bind(("127.0.0.1", 0)) {
        Ok(listener) => {
            let port = listener.local_addr().map_err(|err| err.to_string())?.port();
            listener
                .set_nonblocking(true)
                .map_err(|err| err.to_string())?;
            Ok((port, listener))
        }
        Err(err) => Err(format!("no available local callback port for xAI OAuth: {err}")),
    }
}

fn xai_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("scope", XAI_DEFAULT_SCOPES),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");
    format!(
        "{}/oauth2/authorize?{query}",
        issuer.trim_end_matches('/')
    )
}

fn wait_for_xai_callback(
    listener: TcpListener,
    state: &str,
    paste_slot: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        // Prefer a code the user pasted from the Grok "copy this code" page.
        if let Ok(mut guard) = paste_slot.lock()
            && let Some(code) = guard.take()
        {
            let code = code.trim().to_string();
            if !code.is_empty() {
                return Ok(code);
            }
        }

        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(code) = handle_xai_callback_stream(&mut stream, state)? {
                    return Ok(code);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err(
        "xAI OAuth timed out. If the browser showed a code, copy it and press `p` (or Ctrl+V) in the OAuth modal to paste it."
            .to_string(),
    )
}

fn handle_xai_callback_stream(
    stream: &mut TcpStream,
    state: &str,
) -> std::result::Result<Option<String>, String> {
    let request = read_http_request(stream)?;
    let Some((request_line, _)) = request.split_once("\r\n") else {
        write_html_response(stream, 400, "Invalid request")?;
        return Ok(None);
    };

    let is_callback = request_line.starts_with(&format!("GET {XAI_CALLBACK_PATH}?"))
        || request_line.starts_with("GET /callback?");
    if !is_callback {
        write_html_response(stream, 404, "Not found")?;
        return Ok(None);
    }

    let params = parse_get_query_params(request_line)?;
    if params.get("state").map(String::as_str) != Some(state) {
        write_html_response(stream, 400, "State mismatch")?;
        return Ok(None);
    }
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        write_html_response(stream, 400, "xAI login failed")?;
        return Err(description.to_string());
    }
    let Some(code) = params.get("code").filter(|code| !code.trim().is_empty()) else {
        write_html_response(stream, 400, "Missing authorization code")?;
        return Ok(None);
    };

    write_html_response(
        stream,
        200,
        "xAI / Grok login received. You can return to NAVI.",
    )?;
    Ok(Some(code.clone()))
}

#[derive(Debug, Deserialize)]
struct OpenAiUsagePayload {
    plan_type: Option<String>,
    rate_limit: Option<OpenAiRateLimitDetails>,
    additional_rate_limits: Option<Vec<OpenAiAdditionalRateLimitDetails>>,
    rate_limit_reached_type: Option<OpenAiRateLimitReachedType>,
}

impl OpenAiUsagePayload {
    fn into_report(self) -> OpenAiUsageReport {
        let mut limits = Vec::new();
        if let Some(rate_limit) = self.rate_limit {
            limits.push(rate_limit.into_snapshot(
                Some("codex".to_string()),
                Some("Codex".to_string()),
                Some("codex".to_string()),
            ));
        }
        limits.extend(
            self.additional_rate_limits
                .unwrap_or_default()
                .into_iter()
                .map(OpenAiAdditionalRateLimitDetails::into_snapshot),
        );

        OpenAiUsageReport {
            plan_type: self.plan_type,
            limit_reached_kind: self.rate_limit_reached_type.map(|value| value.kind),
            limits,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiRateLimitDetails {
    #[serde(default)]
    limit_reached: bool,
    primary_window: Option<OpenAiUsageWindow>,
    secondary_window: Option<OpenAiUsageWindow>,
}

impl OpenAiRateLimitDetails {
    fn into_snapshot(
        self,
        limit_id: Option<String>,
        limit_name: Option<String>,
        metered_feature: Option<String>,
    ) -> OpenAiUsageLimitSnapshot {
        OpenAiUsageLimitSnapshot {
            limit_id,
            limit_name,
            metered_feature,
            limit_reached: self.limit_reached,
            primary: self.primary_window,
            secondary: self.secondary_window,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiAdditionalRateLimitDetails {
    limit_name: Option<String>,
    metered_feature: Option<String>,
    rate_limit: Option<OpenAiRateLimitDetails>,
}

impl OpenAiAdditionalRateLimitDetails {
    fn into_snapshot(self) -> OpenAiUsageLimitSnapshot {
        let limit_id = self.metered_feature.clone();
        self.rate_limit
            .unwrap_or(OpenAiRateLimitDetails {
                limit_reached: false,
                primary_window: None,
                secondary_window: None,
            })
            .into_snapshot(limit_id, self.limit_name, self.metered_feature)
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiRateLimitReachedType {
    kind: String,
}

#[derive(Debug, Clone)]
struct PkceCodes {
    code_verifier: String,
    code_challenge: String,
}

impl PkceCodes {
    fn generate() -> Self {
        let code_verifier = generate_oauth_token();
        let digest = Sha256::digest(code_verifier.as_bytes());
        let code_challenge = base64_url_no_pad(&digest);
        Self {
            code_verifier,
            code_challenge,
        }
    }
}

#[derive(Debug, Deserialize)]
struct OpenAiTokenResponse {
    access_token: String,
}

async fn exchange_openai_code_for_tokens(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    code: &str,
) -> std::result::Result<OpenAiTokenResponse, String> {
    let body = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", &pkce.code_verifier),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");

    let response = reqwest::Client::new()
        .post(format!("{}/oauth/token", issuer.trim_end_matches('/')))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("OpenAI token exchange failed: {status}: {body}"));
    }

    response.json().await.map_err(|err| err.to_string())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CommandCodeCallback {
    api_key: String,
    state: String,
    user_id: String,
    user_name: String,
    key_name: String,
}

#[derive(Debug, Deserialize)]
struct CommandCodeCallbackError {
    error: String,
    error_description: Option<String>,
}

pub async fn commandcode_browser_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    mut on_started: F,
) -> std::result::Result<String, String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    let (port, listener) = commandcode_auth_listener()?;
    let state = generate_commandcode_state();
    let auth_url = commandcode_auth_url(port, &state);
    on_started(DeviceOAuthStarted { verification_uri: auth_url, user_code: String::new(), paste_slot: None });

    let callback = tokio::task::spawn_blocking(move || {
        wait_for_commandcode_callback(listener, &state, Duration::from_secs(120))
    })
    .await
    .map_err(|err| err.to_string())??;

    let client = reqwest::Client::new();
    commandcode_get_json(&client, &callback.api_key, "/alpha/whoami")
        .await
        .map_err(|err| format!("Command Code credential validation failed: {err}"))?;

    let account_id = credential_store
        .set_commandcode_credential(
            provider_id,
            &callback.api_key,
            CommandCodeCredentialMetadata {
                user_id: callback.user_id.clone(),
                user_name: callback.user_name.clone(),
                key_name: callback.key_name.clone(),
                authenticated_at: current_unix_timestamp().to_string(),
            },
        )
        .map_err(|err| err.to_string())?;

    Ok(account_id)
}

pub async fn commandcode_fetch_usage_data(
    api_key: &str,
) -> std::result::Result<CommandCodeUsageData, String> {
    let client = reqwest::Client::new();
    let whoami = commandcode_get_json(&client, api_key, "/alpha/whoami").await?;
    let org_id = whoami
        .get("org")
        .and_then(|org| org.get("id"))
        .and_then(Value::as_str);
    let credits_endpoint = commandcode_endpoint_with_params("/alpha/billing/credits", org_id, None);
    let subscription_endpoint =
        commandcode_endpoint_with_params("/alpha/billing/subscriptions", org_id, None);

    let credits = commandcode_get_json(&client, api_key, &credits_endpoint)
        .await
        .ok();
    let subscription = commandcode_get_json(&client, api_key, &subscription_endpoint)
        .await
        .ok();
    let since = subscription
        .as_ref()
        .and_then(|value| value.get("data"))
        .and_then(|data| data.get("currentPeriodStart"))
        .and_then(Value::as_str);
    let usage_endpoint = commandcode_endpoint_with_params("/alpha/usage/summary", org_id, since);
    let usage_summary = commandcode_get_json(&client, api_key, &usage_endpoint)
        .await
        .ok();
    let models = commandcode_list_models(api_key).await.unwrap_or_default();

    Ok(CommandCodeUsageData {
        whoami,
        credits,
        subscription,
        usage_summary,
        models,
    })
}

pub async fn commandcode_list_models(api_key: &str) -> std::result::Result<Vec<String>, String> {
    let client = reqwest::Client::new();
    let value = commandcode_get_json(&client, api_key, "/provider/v1/models").await?;
    let models = value
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing models data".to_string())?
        .iter()
        .filter_map(|item| item.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect::<Vec<_>>();
    Ok(models)
}

async fn commandcode_get_json(
    client: &reqwest::Client,
    api_key: &str,
    endpoint: &str,
) -> std::result::Result<Value, String> {
    let url = format!("{}{}", commandcode_api_base_url(), endpoint);
    let response = client
        .get(url)
        .headers(commandcode_headers(api_key))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    response.json().await.map_err(|err| err.to_string())
}

fn commandcode_headers(api_key: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, USER_AGENT};

    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {api_key}")).expect("valid auth header"),
    );
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("command-code/0.38.2 navi"),
    );
    headers.insert(
        "x-command-code-version",
        HeaderValue::from_static(COMMANDCODE_CLI_VERSION),
    );
    headers
}

fn commandcode_endpoint_with_params(
    endpoint: &str,
    org_id: Option<&str>,
    since: Option<&str>,
) -> String {
    let mut params = Vec::new();
    if let Some(org_id) = org_id {
        params.push(format!("orgId={}", url_encode_component(org_id)));
    }
    if let Some(since) = since {
        params.push(format!("since={}", url_encode_component(since)));
    }
    if params.is_empty() {
        endpoint.to_string()
    } else {
        format!("{}?{}", endpoint, params.join("&"))
    }
}

fn commandcode_api_base_url() -> String {
    std::env::var("COMMANDCODE_API_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| COMMANDCODE_DEFAULT_API_BASE.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn commandcode_auth_listener() -> std::result::Result<(u16, TcpListener), String> {
    for port in 5959..5969 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                listener
                    .set_nonblocking(true)
                    .map_err(|err| err.to_string())?;
                return Ok((port, listener));
            }
            Err(_) => continue,
        }
    }
    Err("no available local callback port for Command Code OAuth".to_string())
}

fn commandcode_auth_url(port: u16, state: &str) -> String {
    let studio_base = std::env::var("COMMANDCODE_STUDIO_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| COMMANDCODE_DEFAULT_STUDIO_BASE.to_string())
        .trim_end_matches('/')
        .to_string();
    format!(
        "{studio_base}/studio/auth/cli?callback=http%3A%2F%2Flocalhost%3A{port}%2Fcallback&state={}",
        url_encode_component(state)
    )
}

fn openai_issuer() -> String {
    std::env::var("NAVI_OPENAI_OAUTH_ISSUER")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| OPENAI_DEFAULT_ISSUER.to_string())
        .trim_end_matches('/')
        .to_string()
}

fn openai_client_id() -> String {
    std::env::var("NAVI_OPENAI_OAUTH_CLIENT_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| OPENAI_DEFAULT_CLIENT_ID.to_string())
}

fn openai_usage_url() -> String {
    std::env::var("NAVI_OPENAI_USAGE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "https://chatgpt.com/backend-api/wham/usage".to_string())
}

fn openai_auth_listener() -> std::result::Result<(u16, TcpListener), String> {
    for port in [1455, 1457].into_iter().chain(5969..5989) {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                listener
                    .set_nonblocking(true)
                    .map_err(|err| err.to_string())?;
                return Ok((port, listener));
            }
            Err(_) => continue,
        }
    }
    Err("no available local callback port for OpenAI OAuth".to_string())
}

fn openai_authorize_url(
    issuer: &str,
    client_id: &str,
    redirect_uri: &str,
    pkce: &PkceCodes,
    state: &str,
) -> String {
    let query = [
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        (
            "scope",
            "openid profile email offline_access api.connectors.read api.connectors.invoke",
        ),
        ("code_challenge", &pkce.code_challenge),
        ("code_challenge_method", "S256"),
        ("id_token_add_organizations", "true"),
        ("codex_cli_simplified_flow", "true"),
        ("state", state),
        ("originator", "navi"),
    ]
    .into_iter()
    .map(|(key, value)| format!("{key}={}", url_encode_component(value)))
    .collect::<Vec<_>>()
    .join("&");
    format!("{}/oauth/authorize?{query}", issuer.trim_end_matches('/'))
}

fn current_unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn url_encode_component(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn generate_oauth_token() -> String {
    let mut bytes = [0u8; 64];
    fill_random_bytes(&mut bytes);
    base64_url_no_pad(&bytes)
}

fn generate_commandcode_state() -> String {
    let mut bytes = [0u8; 32];
    fill_random_bytes(&mut bytes);
    base64_url_no_pad(&bytes)
}

fn fill_random_bytes(bytes: &mut [u8]) {
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .is_ok()
    {
        return;
    }

    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ ((std::process::id() as u128) << 64);
    for chunk in bytes.chunks_mut(16) {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let seed_bytes = seed.to_le_bytes();
        chunk.copy_from_slice(&seed_bytes[..chunk.len()]);
    }
}

fn base64_url_no_pad(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        out.push(TABLE[(b0 >> 2) as usize] as char);
        out.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        }
    }
    out
}

fn wait_for_commandcode_callback(
    listener: TcpListener,
    state: &str,
    timeout: Duration,
) -> std::result::Result<CommandCodeCallback, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(callback) = handle_commandcode_callback_stream(&mut stream, state)? {
                    return Ok(callback);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("Command Code OAuth timed out waiting for browser callback".to_string())
}

fn wait_for_openai_callback(
    listener: TcpListener,
    state: &str,
    timeout: Duration,
) -> std::result::Result<String, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        match listener.accept() {
            Ok((mut stream, _)) => {
                if let Some(code) = handle_openai_callback_stream(&mut stream, state)? {
                    return Ok(code);
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("OpenAI OAuth timed out waiting for browser callback".to_string())
}

fn handle_openai_callback_stream(
    stream: &mut TcpStream,
    state: &str,
) -> std::result::Result<Option<String>, String> {
    let request = read_http_request(stream)?;
    let Some((request_line, _)) = request.split_once("\r\n") else {
        write_html_response(stream, 400, "Invalid request")?;
        return Ok(None);
    };

    if !request_line.starts_with(&format!("GET {OPENAI_CALLBACK_PATH}?")) {
        write_html_response(stream, 404, "Not found")?;
        return Ok(None);
    }

    let params = parse_get_query_params(request_line)?;
    if params.get("state").map(String::as_str) != Some(state) {
        write_html_response(stream, 400, "State mismatch")?;
        return Ok(None);
    }
    if let Some(error) = params.get("error") {
        let description = params
            .get("error_description")
            .map(String::as_str)
            .unwrap_or(error);
        write_html_response(stream, 400, "OpenAI login failed")?;
        return Err(description.to_string());
    }
    let Some(code) = params.get("code").filter(|code| !code.trim().is_empty()) else {
        write_html_response(stream, 400, "Missing authorization code")?;
        return Ok(None);
    };

    write_html_response(
        stream,
        200,
        "OpenAI login received. You can return to NAVI.",
    )?;
    Ok(Some(code.clone()))
}

fn handle_commandcode_callback_stream(
    stream: &mut TcpStream,
    state: &str,
) -> std::result::Result<Option<CommandCodeCallback>, String> {
    let request = read_http_request(stream)?;
    let Some((request_line, body)) = request.split_once("\r\n") else {
        write_json_response(
            stream,
            400,
            r#"{"success":false,"error":"Invalid request"}"#,
        )?;
        return Ok(None);
    };

    if request_line.starts_with("OPTIONS ") {
        write_json_response(stream, 204, "")?;
        return Ok(None);
    }
    if !request_line.starts_with("POST /callback ") {
        write_json_response(stream, 404, r#"{"success":false,"error":"Not found"}"#)?;
        return Ok(None);
    }

    let Some((_, body)) = body.split_once("\r\n\r\n") else {
        write_json_response(
            stream,
            400,
            r#"{"success":false,"error":"Invalid request"}"#,
        )?;
        return Ok(None);
    };

    let value: serde_json::Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(_) => {
            write_json_response(stream, 400, r#"{"success":false,"error":"Invalid JSON"}"#)?;
            return Ok(None);
        }
    };

    if value.get("error").is_some() {
        let error: CommandCodeCallbackError =
            serde_json::from_value(value).map_err(|err| err.to_string())?;
        write_json_response(stream, 200, r#"{"success":true}"#)?;
        return Err(error.error_description.unwrap_or_else(|| error.error));
    }

    let callback: CommandCodeCallback = match serde_json::from_value(value) {
        Ok(callback) => callback,
        Err(_) => {
            write_json_response(
                stream,
                400,
                r#"{"success":false,"error":"Missing required fields"}"#,
            )?;
            return Ok(None);
        }
    };
    if callback.api_key.trim().is_empty()
        || callback.state.trim().is_empty()
        || callback.user_id.trim().is_empty()
        || callback.user_name.trim().is_empty()
        || callback.key_name.trim().is_empty()
    {
        write_json_response(
            stream,
            400,
            r#"{"success":false,"error":"Missing required fields"}"#,
        )?;
        return Ok(None);
    }
    if callback.state != state {
        write_json_response(
            stream,
            403,
            r#"{"success":false,"error":"Invalid state token"}"#,
        )?;
        return Ok(None);
    }

    write_json_response(stream, 200, r#"{"success":true}"#)?;
    Ok(Some(callback))
}

fn read_http_request(stream: &mut TcpStream) -> std::result::Result<String, String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|err| err.to_string())?;
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 1024];
    let header_end;
    loop {
        let read = stream.read(&mut chunk).map_err(|err| err.to_string())?;
        if read == 0 {
            return Err("connection closed before request completed".to_string());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 10_000 {
            return Err("callback request too large".to_string());
        }
        if let Some(index) = find_subslice(&buffer, b"\r\n\r\n") {
            header_end = index + 4;
            break;
        }
    }

    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            line.split_once(':').and_then(|(name, value)| {
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })
        })
        .unwrap_or(0);
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut chunk).map_err(|err| err.to_string())?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 10_000 {
            return Err("callback request too large".to_string());
        }
    }

    String::from_utf8(buffer).map_err(|err| err.to_string())
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn write_json_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
) -> std::result::Result<(), String> {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nAccess-Control-Allow-Origin: https://commandcode.ai\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|err| err.to_string())
}

fn parse_get_query_params(
    request_line: &str,
) -> std::result::Result<HashMap<String, String>, String> {
    let target = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "missing request target".to_string())?;
    let query = target
        .split_once('?')
        .map(|(_, query)| query)
        .unwrap_or_default();
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            Ok((url_decode_component(key)?, url_decode_component(value)?))
        })
        .collect()
}

fn url_decode_component(value: &str) -> std::result::Result<String, String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut iter = value.as_bytes().iter().copied();
    while let Some(byte) = iter.next() {
        match byte {
            b'+' => bytes.push(b' '),
            b'%' => {
                let hi = iter
                    .next()
                    .ok_or_else(|| "invalid percent encoding".to_string())?;
                let lo = iter
                    .next()
                    .ok_or_else(|| "invalid percent encoding".to_string())?;
                bytes.push((hex_value(hi)? << 4) | hex_value(lo)?);
            }
            _ => bytes.push(byte),
        }
    }
    String::from_utf8(bytes).map_err(|err| err.to_string())
}

fn hex_value(byte: u8) -> std::result::Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("invalid percent encoding".to_string()),
    }
}

fn write_html_response(
    stream: &mut TcpStream,
    status: u16,
    body: &str,
) -> std::result::Result<(), String> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        _ => "OK",
    };
    let body = format!("<!doctype html><title>NAVI OAuth</title><p>{body}</p>");
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_oauth_started_fields_are_accessible() {
        let started = DeviceOAuthStarted { verification_uri: "https://github.com/login/device".to_string(), user_code: "ABCD-1234".to_string(), paste_slot: None };

        assert_eq!(started.verification_uri, "https://github.com/login/device");
        assert_eq!(started.user_code, "ABCD-1234");
    }

    #[test]
    fn device_oauth_started_can_be_cloned_via_field_access() {
        // DeviceOAuthStarted does not derive Clone, but its fields are public
        // Strings, so consumers can copy field values as needed.
        let started = DeviceOAuthStarted { verification_uri: "https://example.com/verify".to_string(), user_code: "WXYZ-9999".to_string(), paste_slot: None };

        let uri_copy = started.verification_uri.clone();
        let code_copy = started.user_code.clone();

        assert_eq!(uri_copy, started.verification_uri);
        assert_eq!(code_copy, started.user_code);
    }

    #[test]
    fn device_oauth_started_debug_output_contains_fields() {
        let started = DeviceOAuthStarted { verification_uri: "https://github.com/login/device".to_string(), user_code: "TEST-CODE".to_string(), paste_slot: None };

        let debug = format!("{:?}", started);
        assert!(
            debug.contains("https://github.com/login/device"),
            "debug output should contain verification_uri"
        );
        assert!(
            debug.contains("TEST-CODE"),
            "debug output should contain user_code"
        );
    }

    #[test]
    fn device_oauth_started_accepts_empty_strings() {
        // Edge case: empty fields should not cause construction to fail.
        let started = DeviceOAuthStarted { verification_uri: String::new(), user_code: String::new(), paste_slot: None };

        assert!(started.verification_uri.is_empty());
        assert!(started.user_code.is_empty());
    }

    #[test]
    fn commandcode_auth_url_matches_cli_contract() {
        assert_eq!(
            commandcode_auth_url(5959, "state-token"),
            "https://commandcode.ai/studio/auth/cli?callback=http%3A%2F%2Flocalhost%3A5959%2Fcallback&state=state-token"
        );
    }

    #[test]
    fn xai_authorize_url_uses_pkce_and_loopback() {
        let pkce = PkceCodes {
            code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
        };
        let url = xai_authorize_url(
            "https://auth.x.ai",
            XAI_DEFAULT_CLIENT_ID,
            "http://127.0.0.1:8765/callback",
            &pkce,
            "state-1",
        );
        assert!(url.starts_with("https://auth.x.ai/oauth2/authorize?"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(&format!("client_id={XAI_DEFAULT_CLIENT_ID}")));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A8765%2Fcallback"));
        assert!(url.contains("offline_access"));
    }

    #[test]
    fn is_xai_oauth_access_token_detects_jwt_not_platform_key() {
        assert!(is_xai_oauth_access_token(
            "eyJhbGciOiJFUzI1NiIsInR5cCI6ImF0K2p3dCJ9.payload.signature"
        ));
        assert!(!is_xai_oauth_access_token("xai-platform-api-key-abc"));
        assert!(!is_xai_oauth_access_token(""));
        assert!(!is_xai_oauth_access_token("not-a-jwt"));
    }

    #[test]
    fn commandcode_state_is_base64url_without_padding() {
        let state = generate_commandcode_state();
        assert_eq!(state.len(), 43);
        assert!(
            state
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
    }

    #[test]
    fn base64_url_no_pad_encodes_without_padding() {
        assert_eq!(base64_url_no_pad(b"hello"), "aGVsbG8");
    }

    #[test]
    fn openai_authorize_url_uses_pkce_and_local_callback() {
        let pkce = PkceCodes {
            code_verifier: "verifier".to_string(),
            code_challenge: "challenge".to_string(),
        };
        let url = openai_authorize_url(
            "https://auth.openai.com/",
            "client-id",
            "http://localhost:1455/auth/callback",
            &pkce,
            "state token",
        );

        assert!(url.starts_with("https://auth.openai.com/oauth/authorize?"));
        assert!(url.contains("client_id=client-id"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("state=state%20token"));
        assert!(url.contains("originator=navi"));
    }

    #[test]
    fn pkce_generation_uses_base64url_without_padding() {
        let pkce = PkceCodes::generate();

        assert!(pkce.code_verifier.len() >= 43);
        assert!(pkce.code_challenge.len() >= 43);
        assert!(
            pkce.code_verifier
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
        assert!(
            pkce.code_challenge
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
        );
        assert!(!pkce.code_verifier.contains('='));
        assert!(!pkce.code_challenge.contains('='));
    }

    #[test]
    fn parses_openai_callback_query() {
        let params =
            parse_get_query_params("GET /auth/callback?code=abc%2B123&state=state+token HTTP/1.1")
                .expect("query params");

        assert_eq!(params.get("code").map(String::as_str), Some("abc+123"));
        assert_eq!(params.get("state").map(String::as_str), Some("state token"));
    }

    #[test]
    fn openai_usage_payload_maps_primary_secondary_and_additional_limits() {
        let payload = serde_json::from_str::<OpenAiUsagePayload>(
            r#"{
                "plan_type": "plus",
                "rate_limit": {
                    "limit_reached": true,
                    "primary_window": {
                        "used_percent": 100,
                        "limit_window_seconds": 18000,
                        "reset_after_seconds": 3600,
                        "reset_at": 1700003600
                    },
                    "secondary_window": {
                        "used_percent": 80,
                        "limit_window_seconds": 604800,
                        "reset_after_seconds": 86400,
                        "reset_at": 1700086400
                    }
                },
                "additional_rate_limits": [
                    {
                        "limit_name": "Long context",
                        "metered_feature": "long_context",
                        "rate_limit": {
                            "limit_reached": false,
                            "primary_window": {
                                "used_percent": 10,
                                "limit_window_seconds": 18000,
                                "reset_after_seconds": 1800,
                                "reset_at": 1700001800
                            }
                        }
                    }
                ],
                "rate_limit_reached_type": { "kind": "primary" }
            }"#,
        )
        .expect("usage payload");

        let report = payload.into_report();

        assert_eq!(report.plan_type.as_deref(), Some("plus"));
        assert_eq!(report.limit_reached_kind.as_deref(), Some("primary"));
        assert_eq!(report.limits.len(), 2);
        assert_eq!(report.limits[0].limit_id.as_deref(), Some("codex"));
        assert!(report.limits[0].limit_reached);
        assert_eq!(
            report.limits[0]
                .primary
                .as_ref()
                .map(|window| window.limit_window_seconds),
            Some(18_000)
        );
        assert_eq!(
            report.limits[0]
                .secondary
                .as_ref()
                .map(|window| window.limit_window_seconds),
            Some(604_800)
        );
        assert_eq!(
            report.limits[1].metered_feature.as_deref(),
            Some("long_context")
        );
    }
}
