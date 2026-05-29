use navi_core::CredentialStore;
use std::time::Duration;

#[derive(Debug)]
pub struct DeviceOAuthStarted {
    pub verification_uri: String,
    pub user_code: String,
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
}
