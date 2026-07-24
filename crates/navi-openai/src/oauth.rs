use navi_core::{CredentialStore, XAI_GROK_CLI_OAUTH_KIND};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug)]
pub struct DeviceOAuthStarted {
    pub verification_uri: String,
    pub user_code: String,
    /// Optional slot the TUI can write an authorization code into when the
    /// browser shows "copy this code" instead of redirecting to loopback.
    pub paste_slot: Option<std::sync::Arc<std::sync::Mutex<Option<String>>>>,
}

const OPENAI_DEFAULT_ISSUER: &str = "https://auth.openai.com";
const OPENAI_DEFAULT_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const OPENAI_CALLBACK_PATH: &str = "/auth/callback";

/// xAI Grok CLI public OIDC client (same as official `grok` binary).
const XAI_DEFAULT_ISSUER: &str = "https://auth.x.ai";
const XAI_DEFAULT_CLIENT_ID: &str = "b1a00492-073a-47ea-816f-4c329264a828";
const XAI_CALLBACK_PATH: &str = "/callback";
const XAI_DEFAULT_SCOPES: &str = "openid profile email offline_access grok-cli:access api:access conversations:read conversations:write";
/// Base URL for Grok CLI / Grok Build session tokens (OAuth), not Platform API keys.
///
/// Official `grok` bills subscription quota here (`cli-chat-proxy`), while
/// Platform keys use `https://api.x.ai/v1` (pay-as-you-go).
pub const XAI_GROK_CLI_BASE_URL: &str = "https://cli-chat-proxy.grok.com/v1";
/// Fallback client version sent as `x-grok-client-version` when no installed
/// Grok CLI binary can be discovered. The proxy returns HTTP 426 without this
/// header (or with an outdated value).
///
/// Prefer [`xai_grok_cli_client_version`], which can pick up a newer installed
/// `grok` binary under `~/.grok/downloads/`.
pub const XAI_GROK_CLI_CLIENT_VERSION: &str = "0.2.101";

fn xai_grok_base_url() -> String {
    std::env::var("NAVI_XAI_GROK_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| XAI_GROK_CLI_BASE_URL.to_string())
}
/// Client mode used by the official Grok CLI surface for interactive chat.
pub const XAI_GROK_CLI_CLIENT_MODE: &str = "chat";
/// Client surface used for Grok Build / CLI subscription billing.
pub const XAI_GROK_CLI_CLIENT_SURFACE: &str = "grok-build";
/// Early refresh buffer: refresh when fewer than this many seconds remain.
const XAI_REFRESH_SKEW_SECS: i64 = 300;

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

/// Charm Hyper account credit balance from `GET /v1/credits`.
///
/// Hyper bills in **Hypercredits** (prepaid). FAQ: **1 Hypercredit = $0.05 USD**.
/// Token costs still have USD list rates; session spend can be shown as both USD
/// and Hypercredits (`usd / 0.05`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharmHyperCreditsReport {
    /// Remaining Hypercredits on the account.
    pub balance: f64,
    /// How the balance was obtained (`stream-usage` or `credits-api`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// USD value of one Hypercredit (Charm FAQ, 2026).
pub const HYPERCREDIT_USD: f64 = 0.05;

const HYPER_DEFAULT_BASE_URL: &str = "https://hyper.charm.land";
const OPENROUTER_DEFAULT_BASE_URL: &str = "https://openrouter.ai";

/// Last known Hypercredit balance (process-wide).
///
/// Updated from stream `usage.remaining.hypercredits` and from successful
/// `GET /v1/credits` responses. Kept until a newer value arrives so the Usage
/// modal / footer stay correct across multiple refreshes (do **not** clear on
/// read — concurrent open + after-turn fetch used to race on a one-shot take).
static LAST_KNOWN_HYPERCREDIT_BALANCE: std::sync::Mutex<Option<f64>> = std::sync::Mutex::new(None);

/// Serialize access to the process-wide Hypercredit cache (tests + production).
pub(crate) fn with_hypercredit_balance_lock<T>(f: impl FnOnce(&mut Option<f64>) -> T) -> T {
    let mut guard = LAST_KNOWN_HYPERCREDIT_BALANCE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(&mut guard)
}

pub fn usd_to_hypercredits(usd: f64) -> f64 {
    if HYPERCREDIT_USD <= 0.0 {
        return 0.0;
    }
    usd / HYPERCREDIT_USD
}

pub fn hypercredits_to_usd(credits: f64) -> f64 {
    credits * HYPERCREDIT_USD
}

/// Format Hypercredits with thousands separators (Crush `FormatCredits`).
pub fn format_hypercredits(n: f64) -> String {
    let rounded = if n.is_finite() { n.round() as i64 } else { 0 };
    let negative = rounded < 0;
    let s = rounded.unsigned_abs().to_string();
    if s.len() <= 3 {
        return if negative { format!("-{s}") } else { s };
    }
    let mut first_group = s.len() % 3;
    if first_group == 0 {
        first_group = 3;
    }
    let mut out = String::with_capacity(s.len() + s.len() / 3 + usize::from(negative));
    if negative {
        out.push('-');
    }
    let mut next_comma_at = first_group;
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && i == next_comma_at {
            out.push(',');
            next_comma_at += 3;
        }
        out.push(ch);
    }
    out
}

/// Charm Hyper public base URL (`$HYPER_URL` or `https://hyper.charm.land`).
pub fn hyper_base_url() -> String {
    std::env::var("HYPER_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| HYPER_DEFAULT_BASE_URL.to_string())
}

fn openrouter_base_url() -> String {
    std::env::var("NAVI_OPENROUTER_BASE_URL")
        .ok()
        .map(|v| v.trim().trim_end_matches('/').to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OPENROUTER_DEFAULT_BASE_URL.to_string())
}

/// Store a Hypercredit balance extracted from stream usage metadata.
///
/// Non-authoritative (default): never clobber a known **positive** balance with
/// `0`. Charm Hyper often emits intermediate usage chunks with
/// `remaining.hypercredits: 0` (or empty remaining) before the final sample;
/// treating those as truth made the footer flash `◆ 0` every few turns.
pub fn set_hypercredit_balance(balance: f64) {
    set_hypercredit_balance_inner(balance, /*authoritative*/ false);
}

/// Store a Hypercredit balance from a trusted source (`GET /v1/credits`).
///
/// Unlike [`set_hypercredit_balance`], this **may** set the balance to `0`
/// (account truly depleted).
pub fn set_hypercredit_balance_authoritative(balance: f64) {
    set_hypercredit_balance_inner(balance, /*authoritative*/ true);
}

fn set_hypercredit_balance_inner(balance: f64, authoritative: bool) {
    if !balance.is_finite() || balance < 0.0 {
        return;
    }
    with_hypercredit_balance_lock(|slot| {
        if !authoritative
            && balance == 0.0
            && let Some(prev) = *slot
            && prev > 0.0
        {
            // Keep last known positive; ignore stream zero.
            return;
        }
        *slot = Some(balance);
    });
}

/// Take and clear the cached Hypercredit balance (tests / rare reset paths).
///
/// Production usage reporting should prefer [`peek_hypercredit_balance`] so
/// concurrent modal/after-turn refreshes keep seeing the last known balance.
pub fn take_hypercredit_balance() -> Option<f64> {
    with_hypercredit_balance_lock(|slot| slot.take())
}

/// Peek at the cached Hypercredit balance without clearing it.
pub fn peek_hypercredit_balance() -> Option<f64> {
    with_hypercredit_balance_lock(|slot| *slot)
}

/// Extract `usage.remaining.hypercredits` (or nested variants) and cache it.
///
/// Charm Hyper includes remaining prepaid credits in chat-completions usage:
/// `{ "remaining": { "hypercredits": 1234 } }`.
///
/// Prefer the Hyper-specific keys first. A bare `credits` field is only used
/// when no hypercredits key is present (some gateways alias the name).
pub fn extract_hypercredit_balance_from_usage(usage: &Value) -> Option<f64> {
    let remaining = usage
        .get("remaining")
        .or_else(|| usage.get("remaining_credits"))
        .or_else(|| usage.pointer("/usage/remaining"));
    let balance = remaining
        .and_then(|r| {
            r.get("hypercredits")
                .or_else(|| r.get("hyper_credits"))
                // Prefer explicit remaining keys over a bare "credits" which
                // some payloads use for "credits spent this request" (= 0 mid-stream).
                .or_else(|| r.get("balance"))
                .or_else(|| r.get("remaining"))
                .or_else(|| r.get("credits"))
        })
        .and_then(json_number_as_f64)
        .or_else(|| {
            usage
                .get("hypercredits")
                .or_else(|| usage.get("remaining_hypercredits"))
                .and_then(json_number_as_f64)
        })?;
    if !balance.is_finite() || balance < 0.0 {
        return None;
    }
    // Stream path is non-authoritative (sticky against zero clobber).
    set_hypercredit_balance(balance);
    // Return what is actually cached after sticky policy (may keep previous >0).
    peek_hypercredit_balance()
}

fn json_number_as_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_i64().map(|n| n as f64))
        .or_else(|| value.as_u64().map(|n| n as f64))
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
        .filter(|n| n.is_finite())
}

pub async fn charm_hyper_credits_report(
    api_key: &str,
) -> std::result::Result<CharmHyperCreditsReport, String> {
    // Prefer live HTTP so open/R/after-turn always refresh the true balance.
    // On network/API failure, fall back to the last stream/HTTP sample so the
    // Usage modal never flashes "no credits" when we already know a balance.
    match fetch_hyper_credits_http(api_key).await {
        Ok(balance) => {
            // HTTP is authoritative — including a real zero balance.
            set_hypercredit_balance_authoritative(balance);
            Ok(CharmHyperCreditsReport {
                balance,
                source: Some("credits-api".into()),
            })
        }
        Err(http_err) => {
            if let Some(balance) = peek_hypercredit_balance() {
                return Ok(CharmHyperCreditsReport {
                    balance,
                    source: Some("stream-usage".into()),
                });
            }
            Err(http_err)
        }
    }
}

async fn fetch_hyper_credits_http(api_key: &str) -> std::result::Result<f64, String> {
    let url = format!("{}/v1/credits", hyper_base_url());
    let response = reqwest::Client::new()
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", "navi/0.1.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|err| err.to_string())?;
    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Charm Hyper credits request failed: {status}: {body}"
        ));
    }
    let payload: serde_json::Value = response.json().await.map_err(|err| err.to_string())?;
    parse_hyper_credits_api_balance(&payload)
        .ok_or_else(|| "Charm Hyper credits response missing balance".to_string())
}

/// Parse `balance` from Hyper `GET /v1/credits` payload (several shapes).
fn parse_hyper_credits_api_balance(payload: &Value) -> Option<f64> {
    payload
        .get("balance")
        .and_then(json_number_as_f64)
        .or_else(|| {
            payload
                .get("data")
                .and_then(|d| d.get("balance"))
                .and_then(json_number_as_f64)
        })
        .or_else(|| payload.get("credits").and_then(json_number_as_f64))
        .or_else(|| {
            payload
                .get("remaining")
                .and_then(|r| {
                    r.get("hypercredits")
                        .or_else(|| r.get("hyper_credits"))
                        .or_else(|| r.get("balance"))
                        .or_else(|| r.get("credits"))
                })
                .and_then(json_number_as_f64)
        })
}

pub async fn openrouter_usage_report(
    api_key: &str,
) -> std::result::Result<OpenRouterUsageReport, String> {
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/key", openrouter_base_url()))
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
        .get(format!("{}/billing?format=credits", xai_grok_base_url()))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("Accept", "application/json")
        .header(
            "User-Agent",
            format!("grok/{}", xai_grok_cli_client_version()),
        )
        .header("X-XAI-Token-Auth", "xai-grok-cli")
        .header("x-grok-client-version", xai_grok_cli_client_version())
        .header("x-grok-client-mode", XAI_GROK_CLI_CLIENT_MODE)
        .header("x-grok-client-surface", XAI_GROK_CLI_CLIENT_SURFACE)
        .header("x-grok-client-identifier", xai_client_identifier())
        .header("x-grok-agent-id", xai_agent_id())
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

    on_started(DeviceOAuthStarted {
        verification_uri: auth_url,
        user_code: String::new(),
        paste_slot: None,
    });

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

/// Device-code OIDC login for xAI Grok Build (same path as `grok login --device-auth`).
///
/// Shows a short `user_code` (e.g. `WWG6-9PSY`) and opens
/// `https://accounts.x.ai/oauth2/device?user_code=…`. Polls `auth.x.ai` until
/// the user confirms. This is **not** Platform API-key OAuth and **not** the
/// browser loopback/paste-code flow.
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
        .header("Accept", "application/json")
        // Match official Grok CLI / Grok Build surface so the IdP issues a
        // short user_code (AAAA-BBBB) rather than a platform-style flow.
        .header("x-grok-client-version", xai_grok_cli_client_version())
        .header("x-grok-client-surface", "grok-build")
        .header("User-Agent", "navi/0.1.0")
        .body(device_body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if device_response.status().as_u16() == 404 {
        return Err(
            "xAI device-code endpoint unavailable (404). Set NAVI_XAI_OAUTH_BROWSER=1 for loopback PKCE, or update NAVI."
                .to_string(),
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
    let user_code = device_data
        .get("user_code")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if user_code.is_empty()
        || !user_code
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "xAI device login returned invalid user_code {user_code:?} (expected AAAA-BBBB like grok login). Not using browser paste-code."
        ));
    }
    // Prefer complete URI so the browser lands with the code pre-filled.
    let verification_uri = device_data
        .get("verification_uri_complete")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .or_else(|| {
            device_data
                .get("verification_uri")
                .and_then(|value| value.as_str())
                .map(|base| {
                    if base.contains("user_code=") {
                        base.to_string()
                    } else {
                        format!(
                            "{}{}user_code={}",
                            base,
                            if base.contains('?') { "&" } else { "?" },
                            url_encode_component(&user_code)
                        )
                    }
                })
        })
        .ok_or_else(|| "missing verification URL".to_string())?;
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
            .header("Accept", "application/json")
            .header("x-grok-client-version", xai_grok_cli_client_version())
            .header("x-grok-client-surface", "grok-build")
            .header("User-Agent", "navi/0.1.0")
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

        let tokens: XaiTokenResponse =
            token_response.json().await.map_err(|err| err.to_string())?;
        if tokens.access_token.is_empty() {
            continue;
        }
        store_xai_tokens(&credential_store, provider_id, &tokens)?;
        return Ok(());
    }

    Err("xAI device authorization timed out".to_string())
}

/// Default xAI OAuth entry point used by the TUI.
///
/// Matches official `grok login`: **device-code** by default (user opens
/// `https://accounts.x.ai/oauth2/device?user_code=…`). Set
/// `NAVI_XAI_OAUTH_BROWSER=1` for loopback Authorization Code + PKCE.
/// Legacy: `NAVI_XAI_OAUTH_DEVICE=0` also forces browser.
pub async fn xai_oauth<F>(
    credential_store: CredentialStore,
    provider_id: &str,
    on_started: F,
) -> std::result::Result<(), String>
where
    F: FnMut(DeviceOAuthStarted) + Send,
{
    let force_browser = env_flag_true("NAVI_XAI_OAUTH_BROWSER")
        || std::env::var("NAVI_XAI_OAUTH_DEVICE")
            .map(|value| {
                value == "0"
                    || value.eq_ignore_ascii_case("false")
                    || value.eq_ignore_ascii_case("no")
            })
            .unwrap_or(false);
    if force_browser {
        return xai_browser_oauth(credential_store, provider_id, on_started).await;
    }
    xai_device_oauth(credential_store, provider_id, on_started).await
}

fn env_flag_true(name: &str) -> bool {
    std::env::var(name)
        .map(|value| {
            value == "1" || value.eq_ignore_ascii_case("true") || value.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false)
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

/// Resolve the Grok CLI client version used for `x-grok-client-version`.
///
/// Order:
/// 1. `NAVI_XAI_GROK_CLI_VERSION` override
/// 2. Newest `grok-<semver>-*` binary under `~/.grok/downloads/`
/// 3. [`XAI_GROK_CLI_CLIENT_VERSION`] compile-time fallback
pub fn xai_grok_cli_client_version() -> String {
    use std::sync::OnceLock;
    static VERSION: OnceLock<String> = OnceLock::new();
    VERSION
        .get_or_init(|| {
            if let Ok(v) = std::env::var("NAVI_XAI_GROK_CLI_VERSION") {
                let v = v.trim();
                if !v.is_empty() {
                    return v.to_string();
                }
            }
            if let Some(v) = discover_installed_grok_cli_version() {
                return v;
            }
            XAI_GROK_CLI_CLIENT_VERSION.to_string()
        })
        .clone()
}

fn discover_installed_grok_cli_version() -> Option<String> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    let downloads = std::path::PathBuf::from(home)
        .join(".grok")
        .join("downloads");
    let entries = std::fs::read_dir(downloads).ok()?;
    let mut best: Option<(u64, u64, u64, String)> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // grok-0.2.101-linux-x86_64 / grok-0.2.101
        let Some(rest) = name.strip_prefix("grok-") else {
            continue;
        };
        let ver = rest.split('-').next().unwrap_or(rest);
        let mut parts = ver.split('.');
        let (Some(major_s), Some(minor_s), Some(patch_s)) =
            (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(major), Ok(minor), Ok(patch)) = (
            major_s.parse::<u64>(),
            minor_s.parse::<u64>(),
            patch_s.parse::<u64>(),
        ) else {
            continue;
        };
        let candidate = (major, minor, patch, ver.to_string());
        if best.as_ref().is_none_or(|cur| candidate > *cur) {
            best = Some(candidate);
        }
    }
    best.map(|(_, _, _, v)| v)
}

/// Stable machine-scoped client identifier for Grok CLI headers
/// (`x-grok-client-identifier`).
///
/// Prefer a durable id under NAVI's data dir (like Grok's `~/.grok/agent_id`).
/// Falls back to a deterministic hash of machine-id / hostname so multi-instance
/// NAVI processes on the same host share one fingerprint.
pub fn xai_client_identifier() -> String {
    use std::sync::OnceLock;
    static ID: OnceLock<String> = OnceLock::new();
    ID.get_or_init(load_or_create_xai_client_identifier).clone()
}

/// Stable agent id for Grok CLI headers (`x-grok-agent-id`).
///
/// Uses the same durable id as [`xai_client_identifier`] so multi-instance
/// NAVI processes look like one Grok client machine, matching official CLI
/// multi-window behavior.
pub fn xai_agent_id() -> String {
    xai_client_identifier()
}

fn load_or_create_xai_client_identifier() -> String {
    if let Ok(v) = std::env::var("NAVI_XAI_CLIENT_IDENTIFIER") {
        let v = v.trim();
        if !v.is_empty() {
            return v.to_string();
        }
    }

    // Prefer Grok's own agent_id when present so NAVI and `grok` share identity.
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let grok_agent = std::path::PathBuf::from(&home)
            .join(".grok")
            .join("agent_id");
        if let Ok(raw) = std::fs::read_to_string(&grok_agent) {
            let id = raw.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
    }

    if let Some(data_dir) = navi_data_dir() {
        let path = data_dir.join("xai-client-id");
        if let Ok(raw) = std::fs::read_to_string(&path) {
            let id = raw.trim();
            if !id.is_empty() {
                return id.to_string();
            }
        }
        let id = generate_xai_client_identifier();
        if std::fs::create_dir_all(&data_dir).is_ok() {
            let _ = std::fs::write(&path, format!("{id}\n"));
        }
        return id;
    }

    generate_xai_client_identifier()
}

fn navi_data_dir() -> Option<std::path::PathBuf> {
    if let Ok(v) = std::env::var("NAVI_DATA_DIR") {
        let v = v.trim();
        if !v.is_empty() {
            return Some(std::path::PathBuf::from(v));
        }
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return Some(
            std::path::PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("navi"),
        );
    }
    None
}

fn generate_xai_client_identifier() -> String {
    // Deterministic UUID-shaped id from machine fingerprint (no random dep).
    let mut material = String::from("navi-xai-client");
    if let Ok(mid) = std::fs::read_to_string("/etc/machine-id") {
        material.push(':');
        material.push_str(mid.trim());
    } else if let Ok(host) = std::env::var("HOSTNAME").or_else(|_| std::env::var("COMPUTERNAME")) {
        material.push(':');
        material.push_str(&host);
    }
    let digest = Sha256::digest(material.as_bytes());
    // Format first 16 bytes as 8-4-4-4-12 hex (UUID-like).
    let mut hex = String::with_capacity(32);
    for b in digest.iter().take(16) {
        hex.push(b"0123456789abcdef"[(b >> 4) as usize] as char);
        hex.push(b"0123456789abcdef"[(b & 0x0f) as usize] as char);
    }
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Opaque request id for `x-grok-req-id` / correlation headers.
pub fn xai_new_request_id() -> String {
    static NEXT_XAI_REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let sequence = NEXT_XAI_REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let digest = Sha256::digest(format!("navi-req:{pid}:{nanos}:{sequence}").as_bytes());
    let mut hex = String::with_capacity(32);
    for b in digest.iter().take(16) {
        hex.push(b"0123456789abcdef"[(b >> 4) as usize] as char);
        hex.push(b"0123456789abcdef"[(b & 0x0f) as usize] as char);
    }
    hex
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
        Err(err) => Err(format!(
            "no available local callback port for xAI OAuth: {err}"
        )),
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
    format!("{}/oauth2/authorize?{query}", issuer.trim_end_matches('/'))
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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// std::env::set_var/remove_var are unsafe and racy across threads; serialize
    /// tests that mutate provider-specific env variables.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn with_env_set<F, Fut, T>(key: &str, value: &str, f: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = T>,
    {
        let _guard = ENV_LOCK.lock().await;
        unsafe { std::env::set_var(key, value) };
        let result = f().await;
        unsafe { std::env::remove_var(key) };
        result
    }

    #[test]
    fn device_oauth_started_fields_are_accessible() {
        let started = DeviceOAuthStarted {
            verification_uri: "https://github.com/login/device".to_string(),
            user_code: "ABCD-1234".to_string(),
            paste_slot: None,
        };

        assert_eq!(started.verification_uri, "https://github.com/login/device");
        assert_eq!(started.user_code, "ABCD-1234");
    }

    #[test]
    fn device_oauth_started_can_be_cloned_via_field_access() {
        // DeviceOAuthStarted does not derive Clone, but its fields are public
        // Strings, so consumers can copy field values as needed.
        let started = DeviceOAuthStarted {
            verification_uri: "https://example.com/verify".to_string(),
            user_code: "WXYZ-9999".to_string(),
            paste_slot: None,
        };

        let uri_copy = started.verification_uri.clone();
        let code_copy = started.user_code.clone();

        assert_eq!(uri_copy, started.verification_uri);
        assert_eq!(code_copy, started.user_code);
    }

    #[test]
    fn device_oauth_started_debug_output_contains_fields() {
        let started = DeviceOAuthStarted {
            verification_uri: "https://github.com/login/device".to_string(),
            user_code: "TEST-CODE".to_string(),
            paste_slot: None,
        };

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
        let started = DeviceOAuthStarted {
            verification_uri: String::new(),
            user_code: String::new(),
            paste_slot: None,
        };

        assert!(started.verification_uri.is_empty());
        assert!(started.user_code.is_empty());
    }

    /// Process-wide Hypercredit cache is shared — serialize tests that touch it.
    fn hypercredit_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn format_hypercredits_uses_thousands_separators() {
        assert_eq!(format_hypercredits(42.0), "42");
        assert_eq!(format_hypercredits(999.4), "999");
        assert_eq!(format_hypercredits(1_234.0), "1,234");
        assert_eq!(format_hypercredits(12_345_678.0), "12,345,678");
        assert_eq!(format_hypercredits(-1_500.2), "-1,500");
    }

    #[test]
    fn extract_hypercredit_balance_from_usage_remaining_field() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        let usage = serde_json::json!({
            "prompt_tokens": 100,
            "completion_tokens": 20,
            "remaining": { "hypercredits": 12345.0 }
        });
        let balance = extract_hypercredit_balance_from_usage(&usage).expect("balance");
        assert_eq!(balance, 12345.0);
        // Cache is durable across peeks so concurrent modal refreshes keep working.
        assert_eq!(peek_hypercredit_balance(), Some(12345.0));
        assert_eq!(peek_hypercredit_balance(), Some(12345.0));
        // take is reserved for explicit reset (tests / rare paths).
        assert_eq!(take_hypercredit_balance(), Some(12345.0));
        assert_eq!(peek_hypercredit_balance(), None);
    }

    #[test]
    fn parse_hyper_credits_api_balance_accepts_nested_shapes() {
        assert_eq!(
            parse_hyper_credits_api_balance(&serde_json::json!({ "balance": 42.0 })),
            Some(42.0)
        );
        assert_eq!(
            parse_hyper_credits_api_balance(&serde_json::json!({ "data": { "balance": 99 } })),
            Some(99.0)
        );
        assert_eq!(
            parse_hyper_credits_api_balance(&serde_json::json!({ "credits": 7 })),
            Some(7.0)
        );
        assert_eq!(
            parse_hyper_credits_api_balance(&serde_json::json!({
                "remaining": { "hypercredits": 1234 }
            })),
            Some(1234.0)
        );
    }

    #[test]
    fn extract_hypercredit_balance_accepts_zero() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        let usage = serde_json::json!({
            "remaining": { "hypercredits": 0 }
        });
        // With empty cache, 0 is allowed (first sample).
        assert_eq!(extract_hypercredit_balance_from_usage(&usage), Some(0.0));
        assert_eq!(peek_hypercredit_balance(), Some(0.0));
        assert_eq!(take_hypercredit_balance(), Some(0.0));
    }

    #[test]
    fn extract_hypercredit_balance_rejects_negative_and_non_finite() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        assert_eq!(
            extract_hypercredit_balance_from_usage(&serde_json::json!({
                "remaining": { "hypercredits": -1.0 }
            })),
            None
        );
        assert_eq!(
            extract_hypercredit_balance_from_usage(&serde_json::json!({
                "remaining": { "hypercredits": "not-a-number" }
            })),
            None
        );
        assert_eq!(peek_hypercredit_balance(), None);
    }

    #[test]
    fn format_hypercredits_groups_thousands_for_six_digits() {
        // 100000 has length 6 (multiple of 3) so first_group is forced to 3.
        assert_eq!(format_hypercredits(100_000.0), "100,000");
        assert_eq!(format_hypercredits(-1_234_567.0), "-1,234,567");
    }

    #[test]
    fn set_hypercredit_balance_ignores_negative_and_nan() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        set_hypercredit_balance_authoritative(50.0);
        set_hypercredit_balance(-10.0);
        set_hypercredit_balance(f64::NAN);
        assert_eq!(peek_hypercredit_balance(), Some(50.0));
    }

    #[test]
    fn stream_zero_does_not_clobber_positive_hypercredit_balance() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        let positive = serde_json::json!({
            "remaining": { "hypercredits": 101.0 }
        });
        assert_eq!(
            extract_hypercredit_balance_from_usage(&positive),
            Some(101.0)
        );
        // Intermediate stream chunk with 0 must not wipe the footer to ◆ 0.
        let zero = serde_json::json!({
            "remaining": { "hypercredits": 0 }
        });
        assert_eq!(extract_hypercredit_balance_from_usage(&zero), Some(101.0));
        assert_eq!(peek_hypercredit_balance(), Some(101.0));
        // Authoritative HTTP may set a real zero.
        set_hypercredit_balance_authoritative(0.0);
        assert_eq!(peek_hypercredit_balance(), Some(0.0));
        let _ = take_hypercredit_balance();
    }

    #[test]
    fn hyper_base_url_defaults_and_env_override() {
        // Default when env is unset/empty is the production host.
        // We only assert the helper returns a non-empty https URL shape;
        // env may be set in developer shells, so compare to helper itself.
        let url = hyper_base_url();
        assert!(!url.is_empty());
        assert!(!url.ends_with('/'));
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

    #[test]
    fn usd_and_hypercredit_conversions_are_inverses() {
        // With the default HYPERCREDIT_USD rate, 1 USD -> 10 hypercredits.
        let usd = 5.0;
        let credits = usd_to_hypercredits(usd);
        let usd_back = hypercredits_to_usd(credits);
        assert!(
            (credits - (usd / HYPERCREDIT_USD)).abs() < 1e-9,
            "{credits}"
        );
        assert!((usd_back - usd).abs() < 1e-9);
    }

    #[test]
    fn usd_to_hypercredits_handles_zero_rate() {
        // HYPERCREDIT_USD is a compile-time constant; guard against divide-by-zero.
        assert!(usd_to_hypercredits(0.0).is_finite());
    }

    #[test]
    fn url_decode_component_round_trips_and_decodes() {
        assert_eq!(
            url_decode_component("hello%20world").unwrap(),
            "hello world"
        );
        assert_eq!(url_decode_component("%2B").unwrap(), "+");
        assert!(url_decode_component("%ZZ").is_err());
        assert!(url_decode_component("%").is_err());
    }

    #[test]
    fn find_subslice_locates_needle() {
        assert_eq!(find_subslice(b"hello world", b"world"), Some(6));
        assert_eq!(find_subslice(b"hello", b"world"), None);
        assert_eq!(find_subslice(b"", b"x"), None);
    }

    #[test]
    fn parse_get_query_params_returns_empty_for_missing_query() {
        let params = parse_get_query_params("GET /callback HTTP/1.1").unwrap();
        assert!(params.is_empty());
    }

    #[test]
    fn openai_issuer_and_client_and_url_use_defaults() {
        let issuer = openai_issuer();
        assert!(issuer.starts_with("https://"));
        let client = openai_client_id();
        assert!(!client.is_empty());
        let url = openai_usage_url();
        assert!(url.starts_with("https://"));
    }

    #[test]
    fn xai_issuer_and_client_use_defaults() {
        let issuer = xai_issuer();
        assert!(issuer.starts_with("https://"));
        let client = xai_client_id();
        assert!(!client.is_empty());
    }

    #[test]
    fn generate_oauth_token_is_non_empty_and_url_safe() {
        let token = generate_oauth_token();
        assert!(!token.is_empty());
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn read_http_request_parses_get_and_body() {
        let request = b"GET /callback?code=abc&state=s HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (mut server, mut client) = connected_tcp_pair();
        client.write_all(request).unwrap();
        drop(client);
        let text = read_http_request(&mut server).unwrap();
        assert!(text.starts_with("GET /callback"));

        let request =
            b"POST /callback HTTP/1.1\r\nHost: localhost\r\nContent-Length: 5\r\n\r\nhello";
        let (mut server, mut client) = connected_tcp_pair();
        client.write_all(request).unwrap();
        drop(client);
        let text = read_http_request(&mut server).unwrap();
        assert!(text.contains("hello"));
    }

    #[test]
    fn read_http_request_errors_when_connection_closed_early() {
        let (mut server, client) = connected_tcp_pair();
        drop(client);
        assert!(read_http_request(&mut server).is_err());
    }

    #[test]
    fn write_html_response_formats_http_response() {
        let (mut server, mut client) = connected_tcp_pair();
        thread::spawn(move || {
            let mut buf = [0u8; 512];
            let n = client.read(&mut buf).unwrap();
            String::from_utf8_lossy(&buf[..n]).to_string()
        });
        write_html_response(&mut server, 200, "OK").unwrap();
        // server is dropped after write, closing the connection so the thread returns.
    }

    #[test]
    fn handle_openai_callback_stream_extracts_code() {
        let request =
            b"GET /auth/callback?code=abc%2B123&state=my-state HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (mut server, mut client) = connected_tcp_pair();
        thread::spawn(move || {
            client.write_all(request).unwrap();
            let mut buf = [0u8; 1024];
            let _ = client.read(&mut buf);
        });
        let code = handle_openai_callback_stream(&mut server, "my-state").unwrap();
        assert_eq!(code, Some("abc+123".to_string()));
    }

    #[test]
    fn handle_openai_callback_stream_rejects_errors_and_bad_state() {
        let (mut server, client) = connected_tcp_pair();
        drop(client);
        assert!(handle_openai_callback_stream(&mut server, "s").is_err());

        let request =
            b"GET /auth/callback?code=abc&state=wrong HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (mut server, mut client) = connected_tcp_pair();
        thread::spawn(move || {
            client.write_all(request).unwrap();
            let mut buf = [0u8; 1024];
            let _ = client.read(&mut buf);
        });
        assert_eq!(
            handle_openai_callback_stream(&mut server, "s").unwrap(),
            None
        );

        let request = b"GET /not-callback HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (mut server, mut client) = connected_tcp_pair();
        thread::spawn(move || {
            client.write_all(request).unwrap();
            let mut buf = [0u8; 1024];
            let _ = client.read(&mut buf);
        });
        assert_eq!(
            handle_openai_callback_stream(&mut server, "s").unwrap(),
            None
        );
    }

    #[test]
    fn handle_xai_callback_stream_extracts_code() {
        let request = b"GET /callback?code=abc&state=my-state HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let (mut server, mut client) = connected_tcp_pair();
        thread::spawn(move || {
            client.write_all(request).unwrap();
            let mut buf = [0u8; 1024];
            let _ = client.read(&mut buf);
        });
        let code = handle_xai_callback_stream(&mut server, "my-state").unwrap();
        assert_eq!(code, Some("abc".to_string()));
    }

    #[test]
    fn wait_for_openai_callback_returns_code_from_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            let mut client = TcpStream::connect(("127.0.0.1", port)).unwrap();
            client
                .write_all(b"GET /auth/callback?code=the-code&state=st HTTP/1.1\r\nHost: localhost\r\n\r\n")
                .unwrap();
        });
        let code = wait_for_openai_callback(listener, "st", Duration::from_secs(1)).unwrap();
        assert_eq!(code, "the-code");
    }

    #[test]
    fn wait_for_xai_callback_returns_pasted_code_first() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let _port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let paste_slot =
            std::sync::Arc::new(std::sync::Mutex::new(Some("pasted-code".to_string())));
        let code =
            wait_for_xai_callback(listener, "st", paste_slot, Duration::from_millis(100)).unwrap();
        assert_eq!(code, "pasted-code");
    }

    #[tokio::test]
    async fn exchange_openai_code_for_tokens_posts_and_parses() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "openai-token"
            })))
            .mount(&mock_server)
            .await;

        let pkce = PkceCodes::generate();
        let tokens = exchange_openai_code_for_tokens(
            &mock_server.uri(),
            "client",
            "http://localhost/callback",
            &pkce,
            "code",
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "openai-token");
    }

    #[tokio::test]
    async fn exchange_xai_code_for_tokens_posts_and_parses() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/oauth2/token"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "xai-token"
            })))
            .mount(&mock_server)
            .await;

        let pkce = PkceCodes::generate();
        let tokens = exchange_xai_code_for_tokens(
            &mock_server.uri(),
            "client",
            "http://localhost/callback",
            &pkce,
            "code",
        )
        .await
        .unwrap();
        assert_eq!(tokens.access_token, "xai-token");
    }

    #[tokio::test]
    async fn openai_usage_report_fetches_and_reports() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/usage"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "plan_type": "plus",
                "rate_limit": {
                    "limit_reached": false,
                    "primary_window": { "used_percent": 10, "limit_window_seconds": 100, "reset_after_seconds": 10, "reset_at": 1 },
                    "secondary_window": { "used_percent": 5, "limit_window_seconds": 200, "reset_after_seconds": 20, "reset_at": 2 }
                }
            })))
            .mount(&mock_server)
            .await;

        let url = format!("{}/usage", mock_server.uri());
        let report = with_env_set("NAVI_OPENAI_USAGE_URL", &url, || {
            openai_usage_report("token")
        })
        .await
        .unwrap();
        assert_eq!(report.plan_type.as_deref(), Some("plus"));
    }

    #[tokio::test]
    async fn charm_hyper_credits_report_fetches_balance() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/credits"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "balance": 123.45
            })))
            .mount(&mock_server)
            .await;

        let report = with_env_set("HYPER_URL", &mock_server.uri(), || {
            charm_hyper_credits_report("key")
        })
        .await
        .unwrap();
        assert_eq!(report.balance, 123.45);
        assert_eq!(report.source.as_deref(), Some("credits-api"));
        let _ = take_hypercredit_balance();
    }

    #[tokio::test]
    async fn charm_hyper_credits_report_falls_back_to_cached_balance() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();
        set_hypercredit_balance_authoritative(99.0);

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/credits"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&mock_server)
            .await;

        let report = with_env_set("HYPER_URL", &mock_server.uri(), || {
            charm_hyper_credits_report("key")
        })
        .await
        .unwrap();
        assert_eq!(report.balance, 99.0);
        assert_eq!(report.source.as_deref(), Some("stream-usage"));
        let _ = take_hypercredit_balance();
    }

    #[tokio::test]
    async fn charm_hyper_credits_report_returns_error_when_no_cache() {
        let _guard = hypercredit_test_lock();
        let _ = take_hypercredit_balance();

        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/credits"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let result = with_env_set("HYPER_URL", &mock_server.uri(), || {
            charm_hyper_credits_report("key")
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn fetch_hyper_credits_http_errors_on_non_success_status() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/credits"))
            .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
            .mount(&mock_server)
            .await;

        let result = with_env_set("HYPER_URL", &mock_server.uri(), || {
            fetch_hyper_credits_http("key")
        })
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unauthorized"));
    }

    #[tokio::test]
    async fn openai_usage_report_errors_on_non_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/usage"))
            .respond_with(ResponseTemplate::new(500).set_body_string("bad"))
            .mount(&mock_server)
            .await;

        let url = format!("{}/usage", mock_server.uri());
        let result = with_env_set("NAVI_OPENAI_USAGE_URL", &url, || {
            openai_usage_report("token")
        })
        .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("500"));
    }

    #[tokio::test]
    async fn openrouter_usage_report_fetches_and_reports() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": {
                    "label": "personal",
                    "is_free_tier": true,
                    "usage": 1.5,
                    "usage_daily": 0.5,
                    "usage_weekly": 1.0,
                    "usage_monthly": 1.5,
                    "limit": 10.0,
                    "limit_remaining": 8.5,
                    "limit_reset": "2026-08-01"
                }
            })))
            .mount(&mock_server)
            .await;

        let report = with_env_set("NAVI_OPENROUTER_BASE_URL", &mock_server.uri(), || {
            openrouter_usage_report("key")
        })
        .await
        .unwrap();
        assert_eq!(report.label.as_deref(), Some("personal"));
        assert_eq!(report.is_free_tier, Some(true));
        assert_eq!(report.usage, Some(1.5));
        assert_eq!(report.limit_remaining, Some(8.5));
        assert_eq!(report.limit_reset.as_deref(), Some("2026-08-01"));
    }

    #[tokio::test]
    async fn openrouter_usage_report_errors_on_non_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/key"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&mock_server)
            .await;

        let result = with_env_set("NAVI_OPENROUTER_BASE_URL", &mock_server.uri(), || {
            openrouter_usage_report("key")
        })
        .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn xai_usage_report_fetches_and_reports() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/billing"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "config": {
                    "creditUsagePercent": 42.5,
                    "currentPeriod": { "type": "monthly", "start": "2026-07-01", "end": "2026-07-31" },
                    "productUsage": [{ "product": "grok", "usagePercent": 10.0 }],
                    "prepaidBalance": { "val": 100.0 },
                    "onDemandUsed": { "val": 5.0 },
                    "onDemandCap": { "val": 50.0 },
                    "isUnifiedBillingUser": true
                }
            })))
            .mount(&mock_server)
            .await;

        let report = with_env_set("NAVI_XAI_GROK_BASE_URL", &mock_server.uri(), || {
            xai_usage_report("token")
        })
        .await
        .unwrap();
        assert_eq!(report.credit_usage_percent, Some(42.5));
        assert_eq!(report.period_type.as_deref(), Some("monthly"));
        assert_eq!(report.prepaid_balance, Some(100.0));
        assert_eq!(report.on_demand_used, Some(5.0));
        assert_eq!(report.on_demand_cap, Some(50.0));
        assert_eq!(report.is_unified_billing, Some(true));
        assert_eq!(report.product_usage.len(), 1);
    }

    #[tokio::test]
    async fn xai_usage_report_errors_on_non_success() {
        let mock_server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/billing"))
            .respond_with(ResponseTemplate::new(426))
            .mount(&mock_server)
            .await;

        let result = with_env_set("NAVI_XAI_GROK_BASE_URL", &mock_server.uri(), || {
            xai_usage_report("token")
        })
        .await;
        assert!(result.is_err());
    }

    fn connected_tcp_pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = thread::spawn(move || TcpStream::connect(("127.0.0.1", port)).unwrap());
        let (server, _) = listener.accept().unwrap();
        (server, client.join().unwrap())
    }
}
