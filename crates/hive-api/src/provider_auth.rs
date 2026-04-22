//! Provider-specific authentication flows.
//!
//! Implements GitHub OAuth device flow and provides an extensible pattern
//! for other provider auth flows.

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

/// Response from GitHub's device code request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

/// Response from GitHub's token polling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenPollResponse {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// The GitHub OAuth client ID for Copilot Chat (VS Code's public client ID).
const GITHUB_COPILOT_CLIENT_ID: &str = "Iv1.b507a08c87ecfe98";

/// Request a device code from GitHub for the Copilot OAuth flow.
pub async fn github_device_code_request(client: &reqwest::Client) -> Result<DeviceCodeResponse> {
    let response = client
        .post("https://github.com/login/device/code")
        .header("accept", "application/json")
        .form(&[("client_id", GITHUB_COPILOT_CLIENT_ID), ("scope", "copilot")])
        .send()
        .await
        .context("failed to request device code from GitHub")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("GitHub device code request failed ({status}): {body}"));
    }

    response.json::<DeviceCodeResponse>().await.context("failed to parse device code response")
}

/// Poll GitHub for an access token using the device code.
///
/// Returns the poll response — caller must check if `access_token` is present
/// or if `error` indicates to keep waiting (`authorization_pending`) or stop
/// (`expired_token`, `access_denied`).
pub async fn github_poll_for_token(
    client: &reqwest::Client,
    device_code: &str,
) -> Result<TokenPollResponse> {
    let response = client
        .post("https://github.com/login/oauth/access_token")
        .header("accept", "application/json")
        .form(&[
            ("client_id", GITHUB_COPILOT_CLIENT_ID),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .context("failed to poll GitHub for token")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("GitHub token poll failed ({status}): {body}"));
    }

    response.json::<TokenPollResponse>().await.context("failed to parse token poll response")
}

// ---------------------------------------------------------------------------
// Email OAuth2 Device Code Flows (Gmail & Outlook)
// ---------------------------------------------------------------------------

/// Scopes needed for IMAP/SMTP access via Gmail (default if no services specified).
#[allow(dead_code)]
const GMAIL_SCOPES: &str = "https://mail.google.com/";

/// Scopes needed for IMAP/SMTP access via Outlook.
/// Uses Microsoft Graph API for both send and read (IMAP/SMTP not supported
/// for personal @outlook.com accounts with OAuth2).
const OUTLOOK_SCOPES: &str =
    "offline_access https://graph.microsoft.com/Mail.Send https://graph.microsoft.com/Mail.ReadWrite";

/// Microsoft uses the "common" tenant for multi-tenant device code flows.
const MS_TENANT: &str = "common";

/// Resolve the OAuth client ID for a provider.
///
/// Priority: environment variable → connector keyring override → built-in default.
pub fn resolve_client_id(
    provider: hive_contracts::connectors::ConnectorProvider,
    connector_id: &str,
) -> Option<String> {
    use hive_contracts::connectors::ConnectorProvider;

    const BUILTIN_OUTLOOK: &str = "80b5c82a-8733-4d48-a39c-bde93b9afa39";

    let (env_key, builtin) = match provider {
        ConnectorProvider::Gmail => (
            "HIVEMIND_GOOGLE_CLIENT_ID",
            option_env!("BUILTIN_GOOGLE_CLIENT_ID"),
        ),
        ConnectorProvider::Microsoft => ("HIVEMIND_OUTLOOK_CLIENT_ID", Some(BUILTIN_OUTLOOK)),
        _ => return None,
    };
    std::env::var(env_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| hive_connectors::secrets::load(connector_id, "client_id_override"))
        .or_else(|| builtin.map(|s| s.to_string()))
}

/// Resolve the OAuth client secret for a provider.
///
/// Priority: environment variable → connector-level keyring override → built-in default.
pub fn resolve_client_secret(
    provider: hive_contracts::connectors::ConnectorProvider,
    connector_id: &str,
) -> String {
    use hive_contracts::connectors::ConnectorProvider;

    let (env_key, builtin) = match provider {
        ConnectorProvider::Gmail => (
            "HIVEMIND_GOOGLE_CLIENT_SECRET",
            option_env!("BUILTIN_GOOGLE_CLIENT_SECRET"),
        ),
        // Outlook device code flow doesn't need a client secret
        _ => return String::new(),
    };
    std::env::var(env_key)
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| hive_connectors::secrets::load(connector_id, "client_secret"))
        .or_else(|| builtin.map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Request a device code for a Google OAuth2 flow.
///
/// The `client_id` must be from a Google Cloud project with the device code
/// flow enabled (OAuth consent screen → "TVs and Limited Input devices").
pub async fn google_device_code_request(
    client: &reqwest::Client,
    client_id: &str,
    scopes: &str,
) -> Result<DeviceCodeResponse> {
    let response = client
        .post("https://oauth2.googleapis.com/device/code")
        .form(&[("client_id", client_id), ("scope", scopes)])
        .send()
        .await
        .context("failed to request device code from Google")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Google device code request failed ({status}): {body}"));
    }

    // Google returns `verification_url` (not `verification_uri`)
    let raw: serde_json::Value = response.json().await.context("parse Google device response")?;
    let device_code = raw["device_code"]
        .as_str()
        .ok_or_else(|| anyhow!("missing device_code in Google device response"))?;
    let user_code = raw["user_code"]
        .as_str()
        .ok_or_else(|| anyhow!("missing user_code in Google device response"))?;
    Ok(DeviceCodeResponse {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: raw["verification_url"]
            .as_str()
            .or_else(|| raw["verification_uri"].as_str())
            .unwrap_or("https://www.google.com/device")
            .to_string(),
        expires_in: raw["expires_in"].as_u64().unwrap_or(1800),
        interval: raw["interval"].as_u64().unwrap_or(5),
    })
}

/// Poll Google for an access token using the device code.
pub async fn google_poll_for_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    device_code: &str,
) -> Result<TokenPollResponse> {
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .context("failed to poll Google for token")?;

    let raw: serde_json::Value = response.json().await.context("parse Google token response")?;

    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

/// Refresh a Google access token using a refresh token.
pub async fn google_refresh_token(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    refresh_token: &str,
) -> Result<TokenPollResponse> {
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .context("failed to refresh Google token")?;

    let raw: serde_json::Value = response.json().await.context("parse Google refresh response")?;

    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

/// Request a device code for a Microsoft/Outlook OAuth2 flow.
///
/// The `client_id` must be from an Azure AD app registration with
/// "Allow public client flows" enabled.
pub async fn outlook_device_code_request(
    client: &reqwest::Client,
    client_id: &str,
    scopes: &str,
) -> Result<DeviceCodeResponse> {
    let url = format!("https://login.microsoftonline.com/{MS_TENANT}/oauth2/v2.0/devicecode");

    let response = client
        .post(&url)
        .form(&[("client_id", client_id), ("scope", scopes)])
        .send()
        .await
        .context("failed to request device code from Microsoft")?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!("Microsoft device code request failed ({status}): {body}"));
    }

    let raw: serde_json::Value =
        response.json().await.context("parse Microsoft device response")?;

    let device_code = raw["device_code"]
        .as_str()
        .ok_or_else(|| anyhow!("missing device_code in Microsoft device response"))?;
    let user_code = raw["user_code"]
        .as_str()
        .ok_or_else(|| anyhow!("missing user_code in Microsoft device response"))?;
    Ok(DeviceCodeResponse {
        device_code: device_code.to_string(),
        user_code: user_code.to_string(),
        verification_uri: raw["verification_uri"]
            .as_str()
            .unwrap_or("https://microsoft.com/devicelogin")
            .to_string(),
        expires_in: raw["expires_in"].as_u64().unwrap_or(900),
        interval: raw["interval"].as_u64().unwrap_or(5),
    })
}

/// Poll Microsoft for an access token using the device code.
pub async fn outlook_poll_for_token(
    client: &reqwest::Client,
    client_id: &str,
    device_code: &str,
) -> Result<TokenPollResponse> {
    let url = format!("https://login.microsoftonline.com/{MS_TENANT}/oauth2/v2.0/token");

    let response = client
        .post(&url)
        .form(&[
            ("client_id", client_id),
            ("device_code", device_code),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .context("failed to poll Microsoft for token")?;

    let raw: serde_json::Value = response.json().await.context("parse Microsoft token response")?;

    // Microsoft returns "authorization_pending" in the `error` field
    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

/// Refresh a Microsoft access token using a refresh token.
pub async fn outlook_refresh_token(
    client: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenPollResponse> {
    let url = format!("https://login.microsoftonline.com/{MS_TENANT}/oauth2/v2.0/token");

    let response = client
        .post(&url)
        .form(&[
            ("client_id", client_id),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
            ("scope", OUTLOOK_SCOPES),
        ])
        .send()
        .await
        .context("failed to refresh Microsoft token")?;

    let raw: serde_json::Value =
        response.json().await.context("parse Microsoft refresh response")?;

    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

// ---------------------------------------------------------------------------
// Authorization Code Flow (for Google "Desktop app" client type)
// ---------------------------------------------------------------------------

/// Build Google OAuth authorization URL for the authorization code flow.
pub fn google_build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    state: &str,
    scopes: &str,
) -> String {
    format!(
        "https://accounts.google.com/o/oauth2/v2/auth?\
         client_id={client_id}\
         &redirect_uri={redirect_uri}\
         &response_type=code\
         &scope={}\
         &access_type=offline\
         &prompt=consent\
         &state={state}",
        urlencoding::encode(scopes),
    )
}

/// Exchange a Google authorization code for tokens.
pub async fn google_exchange_auth_code(
    client: &reqwest::Client,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenPollResponse> {
    let response = client
        .post("https://oauth2.googleapis.com/token")
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .context("failed to exchange Google authorization code")?;

    let raw: serde_json::Value =
        response.json().await.context("parse Google auth code exchange response")?;

    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

/// Exchange an authorization code for tokens at an arbitrary token endpoint.
///
/// This is a generic version of `google_exchange_auth_code` that works with
/// any OAuth2 provider that supports the standard authorization-code flow.
pub async fn exchange_auth_code_generic(
    client: &reqwest::Client,
    token_url: &str,
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
) -> Result<TokenPollResponse> {
    let response = client
        .post(token_url)
        .form(&[
            ("client_id", client_id),
            ("client_secret", client_secret),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .with_context(|| format!("failed to exchange authorization code at {token_url}"))?;

    let raw: serde_json::Value =
        response.json().await.context("parse auth code exchange response")?;

    Ok(TokenPollResponse {
        access_token: raw["access_token"].as_str().map(|s| s.to_string()),
        token_type: raw["token_type"].as_str().map(|s| s.to_string()),
        scope: raw["scope"].as_str().map(|s| s.to_string()),
        refresh_token: raw["refresh_token"].as_str().map(|s| s.to_string()),
        error: raw["error"].as_str().map(|s| s.to_string()),
        error_description: raw["error_description"].as_str().map(|s| s.to_string()),
    })
}

/// Start a temporary local HTTP server to receive the OAuth callback.
///
/// Returns `(port, receiver)`. The receiver will yield the authorization code
/// (or an error string) once the user completes the browser-based flow.
/// The server automatically shuts down after receiving one callback.
pub async fn start_oauth_callback_server(
) -> Result<(u16, tokio::sync::oneshot::Receiver<Result<String>>)> {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .context("failed to bind local OAuth callback server")?;
    let port = listener.local_addr()?.port();

    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String>>();
    let tx = std::sync::Arc::new(tokio::sync::Mutex::new(Some(tx)));

    tokio::spawn(async move {
        // Accept exactly one connection
        if let Ok((mut stream, _addr)) = listener.accept().await {
            let tx = tx.clone();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 4096];
            if let Ok(n) = stream.read(&mut buf).await {
                let request = String::from_utf8_lossy(&buf[..n]);
                // Parse the GET request line: GET /callback?code=xxx&state=yyy HTTP/1.1
                let auth_code = parse_callback_code(&request);

                // Send a nice HTML response to the browser
                let (status, body) = match &auth_code {
                    Ok(_) => ("200 OK", "<html><body style='font-family:system-ui;text-align:center;padding:3rem;background:#1a1a2e;color:#e5e5e5'><h1>✅ Authorization successful!</h1><p>You can close this tab and return to HiveMind OS.</p></body></html>".to_string()),
                    Err(e) => ("400 Bad Request", format!("<html><body style='font-family:system-ui;text-align:center;padding:3rem;background:#1a1a2e;color:#e5e5e5'><h1>❌ Authorization failed</h1><p>{e}</p></body></html>")),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{body}"
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;

                // Send the code through the channel
                if let Some(sender) = tx.lock().await.take() {
                    let _ = sender.send(auth_code);
                }
            }
        }
    });

    Ok((port, rx))
}

/// Parse the authorization code from an HTTP callback request.
fn parse_callback_code(request: &str) -> Result<String> {
    // Extract the request path from the first line
    let first_line = request.lines().next().unwrap_or("");
    let path = first_line.split_whitespace().nth(1).unwrap_or("");

    // Parse query parameters
    let query = path.split('?').nth(1).unwrap_or("");
    let mut code = None;
    let mut error = None;
    for param in query.split('&') {
        let mut kv = param.splitn(2, '=');
        match (kv.next(), kv.next()) {
            (Some("code"), Some(v)) => code = Some(urlencoding::decode(v)?.into_owned()),
            (Some("error"), Some(v)) => error = Some(urlencoding::decode(v)?.into_owned()),
            _ => {}
        }
    }

    if let Some(err) = error {
        return Err(anyhow!("OAuth error: {err}"));
    }
    code.ok_or_else(|| anyhow!("no authorization code in callback"))
}
