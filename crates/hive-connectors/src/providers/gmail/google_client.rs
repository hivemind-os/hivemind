use anyhow::{bail, Context, Result};
use parking_lot::RwLock;
use tracing::debug;

const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// Builtin Google OAuth client ID, injected at compile time.
pub fn builtin_google_client_id() -> Option<&'static str> {
    option_env!("BUILTIN_GOOGLE_CLIENT_ID")
}

/// Shared Google API client with automatic OAuth2 token refresh.
///
/// Unlike `GraphClient`, methods accept full URLs because Google APIs
/// (Gmail, Calendar, Drive, People) each have their own base URL.
pub struct GoogleClient {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    connector_id: String,
    refresh_token: RwLock<String>,
    access_token: RwLock<Option<String>>,
    scopes: String,
}

impl GoogleClient {
    pub fn new(
        connector_id: &str,
        client_id: &str,
        client_secret: &str,
        refresh_token: &str,
        access_token: Option<&str>,
        scopes: &str,
    ) -> Self {
        Self {
            client: reqwest::Client::new(),
            client_id: if client_id.is_empty() {
                builtin_google_client_id().unwrap_or_default().to_string()
            } else {
                client_id.to_string()
            },
            client_secret: client_secret.to_string(),
            connector_id: connector_id.to_string(),
            refresh_token: RwLock::new(refresh_token.to_string()),
            access_token: RwLock::new(access_token.map(|s| s.to_string())),
            scopes: scopes.to_string(),
        }
    }

    /// Get the connector ID.
    pub fn connector_id(&self) -> &str {
        &self.connector_id
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
            .post(GOOGLE_TOKEN_ENDPOINT)
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", &rt),
                ("grant_type", "refresh_token"),
                ("scope", &self.scopes),
            ])
            .send()
            .await
            .context("Google token refresh request failed")?;

        let status = resp.status();
        let raw = resp.text().await.context("reading token response")?;
        let body: serde_json::Value = serde_json::from_str(&raw).unwrap_or_else(|_| {
            if status.is_success() {
                serde_json::Value::Null
            } else {
                serde_json::json!({ "error": format!("token refresh failed ({status}): {raw}") })
            }
        });

        if let Some(new_token) = body["access_token"].as_str() {
            if new_token.trim().is_empty() {
                bail!("token refresh failed: empty access token");
            }
            debug!(
                connector = %self.connector_id,
                "Google API token refreshed"
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

    /// GET request to a full URL. Auto-retries once on 401 with a token refresh.
    pub async fn get(&self, url: &str) -> Result<serde_json::Value> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Google API GET request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .get(url)
                .bearer_auth(&token)
                .send()
                .await
                .context("Google API GET retry failed")?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                bail!("Google API GET {url} failed ({status}): {body}");
            }
            return resp.json().await.context("parsing Google API response");
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Google API GET {url} failed ({status}): {body}");
        }
        resp.json().await.context("parsing Google API response")
    }

    /// POST with JSON body. Auto-retries once on 401.
    pub async fn post(&self, url: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .post(url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Google API POST request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .post(url)
                .bearer_auth(&token)
                .json(body)
                .send()
                .await
                .context("Google API POST retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// PATCH with JSON body. Auto-retries once on 401.
    pub async fn patch(&self, url: &str, body: &serde_json::Value) -> Result<reqwest::Response> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .patch(url)
            .bearer_auth(&token)
            .json(body)
            .send()
            .await
            .context("Google API PATCH request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .patch(url)
                .bearer_auth(&token)
                .json(body)
                .send()
                .await
                .context("Google API PATCH retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// DELETE request. Auto-retries once on 401.
    pub async fn delete(&self, url: &str) -> Result<reqwest::Response> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .delete(url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Google API DELETE request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .delete(url)
                .bearer_auth(&token)
                .send()
                .await
                .context("Google API DELETE retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// PUT with raw bytes (for file upload). Auto-retries once on 401.
    pub async fn put_bytes(
        &self,
        url: &str,
        content: &[u8],
        content_type: &str,
    ) -> Result<reqwest::Response> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .put(url)
            .bearer_auth(&token)
            .header("Content-Type", content_type)
            .body(content.to_vec())
            .send()
            .await
            .context("Google API PUT request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .put(url)
                .bearer_auth(&token)
                .header("Content-Type", content_type)
                .body(content.to_vec())
                .send()
                .await
                .context("Google API PUT retry failed")?;
            return Ok(resp);
        }
        Ok(resp)
    }

    /// POST with multipart form data (for Drive upload).
    ///
    /// Multipart forms are consumed on send, so 401 retry is not possible.
    /// A proactive token refresh is performed before the request to minimise
    /// the chance of hitting an expired token.
    pub async fn post_multipart(
        &self,
        url: &str,
        form: reqwest::multipart::Form,
    ) -> Result<reqwest::Response> {
        // Proactively refresh to avoid a 401 we can't retry.
        let token = self.refresh().await.or_else(|_| {
            // Fall back to cached token if refresh fails (e.g. no refresh_token).
            self.access_token
                .read()
                .clone()
                .filter(|t| !t.is_empty())
                .ok_or_else(|| anyhow::anyhow!("no valid token available"))
        })?;

        let resp = self
            .client
            .post(url)
            .bearer_auth(&token)
            .multipart(form)
            .send()
            .await
            .context("Google API multipart POST request failed")?;

        Ok(resp)
    }

    /// GET request that returns raw bytes (for file downloads).
    /// Auto-retries once on 401 with a token refresh.
    pub async fn get_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let token = self.get_token().await?;

        let resp = self
            .client
            .get(url)
            .bearer_auth(&token)
            .send()
            .await
            .context("Google API GET bytes request failed")?;

        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let token = self.refresh().await?;
            let resp = self
                .client
                .get(url)
                .bearer_auth(&token)
                .send()
                .await
                .context("Google API GET bytes retry failed")?;
            if !resp.status().is_success() {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                bail!("Google API GET {url} failed ({status}): {body}");
            }
            return Ok(resp.bytes().await.context("reading bytes")?.to_vec());
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            bail!("Google API GET {url} failed ({status}): {body}");
        }
        Ok(resp.bytes().await.context("reading bytes")?.to_vec())
    }
}
