use navi_core::{CommandCodeCredentialMetadata, CredentialStore};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::Duration;

#[derive(Debug)]
pub struct DeviceOAuthStarted {
    pub verification_uri: String,
    pub user_code: String,
}

const COMMANDCODE_DEFAULT_API_BASE: &str = "https://api.commandcode.ai";
const COMMANDCODE_DEFAULT_STUDIO_BASE: &str = "https://commandcode.ai";
const COMMANDCODE_CLI_VERSION: &str = "0.38.2";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandCodeUsageData {
    pub whoami: Value,
    pub credits: Option<Value>,
    pub subscription: Option<Value>,
    pub usage_summary: Option<Value>,
    pub models: Vec<String>,
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
    on_started(DeviceOAuthStarted {
        verification_uri: auth_url,
        user_code: String::new(),
    });

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

fn generate_commandcode_state() -> String {
    let mut bytes = [0u8; 32];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_err()
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let pid = std::process::id() as u128;
        bytes[..16].copy_from_slice(&now.to_le_bytes());
        bytes[16..].copy_from_slice(&(now ^ pid).to_le_bytes());
    }
    base64_url_no_pad(&bytes)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_oauth_started_fields_are_accessible() {
        let started = DeviceOAuthStarted {
            verification_uri: "https://github.com/login/device".to_string(),
            user_code: "ABCD-1234".to_string(),
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
        };

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
}
