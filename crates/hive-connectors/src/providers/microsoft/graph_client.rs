use anyhow::{bail, Context, Result};
use parking_lot::RwLock;
use tracing::debug;

const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";
const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";

/// Builtin Outlook client ID.
pub const BUILTIN_OUTLOOK_CLIENT_ID: &str = "80b5c82a-8733-4d48-a39c-bde93b9afa39";

/// Shared Microsoft Graph API client with automatic token refresh.
pub struct GraphClient {
    client: reqwest::Client,
    client_id: String,
    connector_id: String,
    refresh_token: RwLock<String>,
    access_token: RwLock<Option<String>>,
    scopes: String,
}

impl GraphClient {
    pub fn new(
        connector_id: &str,
        client_id: &str,
        refresh_token: &str,
        access_token: Option<&str>,
        scopes: &str,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id: if client_id.is_empty() {
                BUILTIN_OUTLOOK_CLIENT_ID.to_string()
            } else {
                client_id.to_string()
            },
            connector_id: connector_id.to_string(),
            refresh_token: RwLock::new(refresh_token.to_string()),
            access_token: RwLock::new(access_token.map(|s| s.to_string())),
            scopes: scopes.to_string(),
        }
    }

    /// Get a valid access token, refreshing if needed.
    pub async fn get_token(&self) -> Result<String> {
        if let Some(token) = self.access_token.read().clone() {
            if !token.is_empty() {
                return Ok(token);
            }
        }
        self.refresh().await
    }

    /// Force a token refresh.
    pub async fn refresh(&self) -> Result<String> {
        let rt = self.refresh_token.read().clone();
        if rt.is_empty() {
            if let Some(at) = self.access_token.read().clone() {
                if !at.is_empty() {
                    return Ok(at);
                }
            }
            bail!(
                "no OAuth tokens for connector '{}'. Please re-authorize via Settings → Connectors.",
                self.connector_id
            );
        }

        let resp = self
            .client
            .post(TOKEN_ENDPOINT)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("refresh_token", &rt),
                ("grant_type", "refresh_token"),
                ("scope", &self.scopes),
            ])
            .send()
            .await
            .context("token refresh request failed")?;

        let body: serde_json::Value = resp.json().await.context("parsing token response")?;

        if let Some(new_token) = body["access_token"].as_str() {
            if new_token.trim().is_empty() {
                bail!("token refresh failed: empty access token");
            }
            debug!(
                connector = %self.connector_id,
                "Microsoft Graph token refreshed"
            );
            *self.access_token.write() = Some(new_token.to_string());
            if let Some(new_refresh) = body["refresh_token"].as_str() {
                if !new_refresh.is_empty() {
                    *self.refresh_token.write() = new_refresh.to_string();
                }
            }
            crate::secrets::save(&self.connector_id, "access_token", new_token);
            if let Some(new_refresh) = body["refresh_token"].as_str() {
                if !new_refresh.is_empty() {
                    crate::secrets::save(&self.connector_id, "refresh_token", new_refresh);
                }
            }
            Ok(new_token.to_string())
        } else {
            let error_desc = body["error_description"]
                .as_str()
                .or_else(|| body["error"].as_str())
                .unwrap_or("unknown error");
            bail!("token refresh failed: {error_desc}")
        }
    }

    /// Make a GET request to a Graph API endpoint (relative to GRAPH_BASE).
    /// Automatically retries once on 401 with a token refresh.
    pub async fn get(&self, path: &str) -> Result<serde_json::Value> {
        let url = format!("{GRAPH_BASE}{path}");
        let token = self.get_token().await?;

        let resp = self
            .client
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Graph API GET request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .get(&url)
                .bearer_auth(&token)
                .send()
                .await
                .context("Graph API GET retry failed")?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                bail!("Graph API GET {path} failed ({status}): {body}");
            }
            return resp.json().await.context("parsing Graph response");
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Graph API GET {path} failed ({status}): {body}");
        }
        resp.json().await.context("parsing Graph response")
    }

    /// Make a POST request with JSON body.
    pub async fn post(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        let url = format!("{GRAPH_BASE}{path}");
        let token = self.get_token().await?;

        let resp = self
            .client
            .post(&url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Graph API POST request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .post(&url)
                .bearer_auth(&token)
                .json(body)
                .send()
                .await
                .context("Graph API POST retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// Make a PATCH request with JSON body.
    pub async fn patch(&self, path: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        let url = format!("{GRAPH_BASE}{path}");
        let token = self.get_token().await?;

        let resp = self
            .client
            .patch(&url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Graph API PATCH request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .patch(&url)
                .bearer_auth(&token)
                .json(body)
                .send()
                .await
                .context("Graph API PATCH retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// Make a DELETE request.
    pub async fn delete(&self, path: &str) -> Result<reqwest::Response> {
        let url = format!("{GRAPH_BASE}{path}");
        let token = self.get_token().await?;

        let resp = self
            .client
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Graph API DELETE request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .delete(&url)
                .bearer_auth(&token)
                .send()
                .await
                .context("Graph API DELETE retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// Make a PUT request with raw bytes (for file upload).
    pub async fn put_bytes(
        &self,
        path: &str,
        content: &[u8],
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let url = format!("{GRAPH_BASE}{path}");
        let token = self.get_token().await?;

        let resp = self
            .client
            .put(&url)
            .bearer_auth(&token)
            .header("Content-Type", content_type)
            .body(content.to_vec())
            .send()
            .await
            .context("Graph API PUT request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .put(&url)
                .bearer_auth(&token)
                .header("Content-Type", content_type)
                .body(content.to_vec())
                .send()
                .await
                .context("Graph API PUT retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// Get the connector ID.
    pub fn connector_id(&self) -> &str {
        &self.connector_id
    }
}
