use std::time::Duration;

use anyhow::Result;
use backon::{BackoffBuilder, ExponentialBuilder};
use serde::Deserialize;
use tracing::warn;

use crate::services::communication::CommAttachment;

const BASE_URL: &str = "https://discord.com/api/v10";

/// The default Discord API base URL.
pub fn default_base_url() -> &'static str {
    BASE_URL
}

// ---------------------------------------------------------------------------
// Error types
// ---------------------------------------------------------------------------

/// Typed error for Discord API responses, enabling retry decisions.
#[derive(Debug, thiserror::Error)]
pub enum DiscordApiError {
    /// 429 — rate limited, should retry after the given duration.
    #[error("rate limited, retry after {retry_after:?}")]
    RateLimited { retry_after: Duration },
    /// 401 — authentication failed, do NOT retry.
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    /// 5xx or network error — transient, can retry.
    #[error("transient error: {0}")]
    Transient(String),
    /// 4xx (non-401, non-429) — permanent, don't retry.
    #[error("permanent error: {0}")]
    Permanent(String),
}

/// Discord 429 response body shape.
#[derive(Deserialize)]
struct RateLimitBody {
    retry_after: f64,
}

// ---------------------------------------------------------------------------
// Response types — only the fields we need
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct Guild {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Channel {
    pub id: String,
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub type_: u8,
}

#[derive(Debug, Clone, Deserialize)]
struct MessageResponse {
    id: String,
}

#[derive(Debug, Deserialize)]
struct GatewayBotResponse {
    url: String,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn auth_header(bot_token: &str) -> String {
    format!("Bot {bot_token}")
}

/// Classify an HTTP response into success or a typed error.
async fn classify_response(resp: reqwest::Response) -> Result<reqwest::Response, DiscordApiError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }

    let body = resp.text().await.unwrap_or_default();

    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = serde_json::from_str::<RateLimitBody>(&body)
            .map(|r| Duration::from_secs_f64(r.retry_after))
            .unwrap_or(Duration::from_secs(1));
        return Err(DiscordApiError::RateLimited { retry_after });
    }

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(DiscordApiError::AuthFailed(body));
    }

    if status.is_server_error() {
        return Err(DiscordApiError::Transient(format!("Discord API error {status}: {body}")));
    }

    Err(DiscordApiError::Permanent(format!("Discord API error {status}: {body}")))
}

/// Maximum number of retry attempts (applies to both rate-limit and transient errors).
const MAX_RETRIES: u32 = 5;

/// Execute a Discord API operation with automatic retry on transient and rate-limit errors.
///
/// - Rate-limited (429): sleeps for the `retry_after` duration specified by Discord.
/// - Transient (5xx/network): retries with exponential backoff (1s–30s).
/// - Auth failures and permanent 4xx errors: returned immediately without retry.
async fn with_retry<F, Fut, T>(op: F) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, DiscordApiError>>,
{
    let mut backoff = ExponentialBuilder::default()
        .with_min_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(30))
        .with_max_times(MAX_RETRIES as usize)
        .build();

    let mut attempts = 0u32;

    loop {
        match op().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempts += 1;
                match e {
                    DiscordApiError::RateLimited { retry_after } => {
                        if attempts > MAX_RETRIES {
                            return Err(DiscordApiError::RateLimited { retry_after }.into());
                        }
                        warn!(
                            retry_after_ms = retry_after.as_millis() as u64,
                            attempts, "Discord rate limited, waiting"
                        );
                        tokio::time::sleep(retry_after).await;
                    }
                    DiscordApiError::Transient(msg) => {
                        if let Some(delay) = backoff.next() {
                            warn!(
                                %msg,
                                delay_ms = delay.as_millis() as u64,
                                attempts,
                                "transient Discord error, retrying"
                            );
                            tokio::time::sleep(delay).await;
                        } else {
                            return Err(DiscordApiError::Transient(msg).into());
                        }
                    }
                    other => return Err(other.into()),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Public API functions
// ---------------------------------------------------------------------------

/// List guilds the bot is a member of.
pub async fn list_guilds(client: &reqwest::Client, bot_token: &str) -> Result<Vec<Guild>> {
    list_guilds_with_url(client, bot_token, BASE_URL).await
}

/// List guilds using a custom base URL (for testing).
pub async fn list_guilds_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    base_url: &str,
) -> Result<Vec<Guild>> {
    with_retry(|| async {
        let url = format!("{base_url}/users/@me/guilds");
        let resp = client
            .get(&url)
            .header("Authorization", auth_header(bot_token))
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        let resp = classify_response(resp).await?;
        resp.json::<Vec<Guild>>().await.map_err(|e| DiscordApiError::Transient(e.to_string()))
    })
    .await
}

/// List channels in a guild.
pub async fn list_guild_channels(
    client: &reqwest::Client,
    bot_token: &str,
    guild_id: &str,
) -> Result<Vec<Channel>> {
    list_guild_channels_with_url(client, bot_token, guild_id, BASE_URL).await
}

/// List channels in a guild using a custom base URL (for testing).
pub async fn list_guild_channels_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    guild_id: &str,
    base_url: &str,
) -> Result<Vec<Channel>> {
    with_retry(|| async {
        let url = format!("{base_url}/guilds/{guild_id}/channels");
        let resp = client
            .get(&url)
            .header("Authorization", auth_header(bot_token))
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        let resp = classify_response(resp).await?;
        resp.json::<Vec<Channel>>().await.map_err(|e| DiscordApiError::Transient(e.to_string()))
    })
    .await
}

/// Send a text message to a channel. Returns the new message ID.
pub async fn send_message(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
) -> Result<String> {
    send_message_with_url(client, bot_token, channel_id, content, BASE_URL).await
}

/// Send a text message using a custom base URL (for testing).
pub async fn send_message_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    base_url: &str,
) -> Result<String> {
    send_message_rich_with_url(client, bot_token, channel_id, content, None, base_url).await
}

/// Send a message with optional interactive components (action rows with buttons).
pub async fn send_message_rich(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    components: Option<serde_json::Value>,
) -> Result<String> {
    send_message_rich_with_url(client, bot_token, channel_id, content, components, BASE_URL).await
}

/// Send a message with optional components using a custom base URL.
pub async fn send_message_rich_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    components: Option<serde_json::Value>,
    base_url: &str,
) -> Result<String> {
    let components = components.clone();
    with_retry(|| {
        let components = components.clone();
        async move {
            let url = format!("{base_url}/channels/{channel_id}/messages");
            let mut body = serde_json::json!({ "content": content });
            if let Some(comps) = components {
                body["components"] = comps;
            }

            let resp = client
                .post(&url)
                .header("Authorization", auth_header(bot_token))
                .json(&body)
                .send()
                .await
                .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

            let resp = classify_response(resp).await?;
            let msg: MessageResponse =
                resp.json().await.map_err(|e| DiscordApiError::Transient(e.to_string()))?;
            Ok(msg.id)
        }
    })
    .await
}

/// Send a message with file attachments using multipart form data.
pub async fn send_message_with_attachments(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    attachments: &[CommAttachment],
) -> Result<String> {
    send_message_with_attachments_url(client, bot_token, channel_id, content, attachments, BASE_URL)
        .await
}

/// Send a message with attachments using a custom base URL (for testing).
pub async fn send_message_with_attachments_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    content: &str,
    attachments: &[CommAttachment],
    base_url: &str,
) -> Result<String> {
    with_retry(|| async {
        let url = format!("{base_url}/channels/{channel_id}/messages");

        let attachment_meta: Vec<serde_json::Value> = attachments
            .iter()
            .enumerate()
            .map(|(i, att)| {
                serde_json::json!({
                    "id": i,
                    "filename": att.filename,
                })
            })
            .collect();

        let payload = serde_json::json!({
            "content": content,
            "attachments": attachment_meta,
        });

        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| DiscordApiError::Permanent(e.to_string()))?;

        let mut form = reqwest::multipart::Form::new().text("payload_json", payload_json);

        for (i, att) in attachments.iter().enumerate() {
            let part = reqwest::multipart::Part::bytes(att.data.clone())
                .file_name(att.filename.clone())
                .mime_str(&att.media_type)
                .unwrap_or_else(|_| {
                    reqwest::multipart::Part::bytes(att.data.clone())
                        .file_name(att.filename.clone())
                });
            form = form.part(format!("files[{i}]"), part);
        }

        let resp = client
            .post(&url)
            .header("Authorization", auth_header(bot_token))
            .multipart(form)
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        let resp = classify_response(resp).await?;
        let msg: MessageResponse =
            resp.json().await.map_err(|e| DiscordApiError::Transient(e.to_string()))?;
        Ok(msg.id)
    })
    .await
}

/// Get the WebSocket gateway URL for a bot.
pub async fn get_gateway_url(client: &reqwest::Client, bot_token: &str) -> Result<String> {
    get_gateway_url_with_url(client, bot_token, BASE_URL).await
}

/// Get the WebSocket gateway URL using a custom base URL (for testing).
pub async fn get_gateway_url_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    base_url: &str,
) -> Result<String> {
    with_retry(|| async {
        let url = format!("{base_url}/gateway/bot");
        let resp = client
            .get(&url)
            .header("Authorization", auth_header(bot_token))
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        let resp = classify_response(resp).await?;
        let gw: GatewayBotResponse =
            resp.json().await.map_err(|e| DiscordApiError::Transient(e.to_string()))?;
        Ok(gw.url)
    })
    .await
}

// ---------------------------------------------------------------------------
// Interaction callback — must be sent within 3 s of receiving INTERACTION_CREATE
// ---------------------------------------------------------------------------

/// Acknowledge a Discord component interaction with type 7 (UPDATE_MESSAGE).
/// Edits the original interaction message with updated content (e.g. "✅ Resolved").
pub async fn acknowledge_interaction(
    client: &reqwest::Client,
    interaction_id: &str,
    interaction_token: &str,
    update_content: &str,
) -> Result<()> {
    acknowledge_interaction_with_url(
        client,
        interaction_id,
        interaction_token,
        update_content,
        BASE_URL,
    )
    .await
}

pub async fn acknowledge_interaction_with_url(
    client: &reqwest::Client,
    interaction_id: &str,
    interaction_token: &str,
    update_content: &str,
    base_url: &str,
) -> Result<()> {
    with_retry(|| async {
        let url = format!("{base_url}/interactions/{interaction_id}/{interaction_token}/callback");
        let body = serde_json::json!({
            "type": 7,
            "data": {
                "content": update_content,
                "components": [],
            }
        });

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        classify_response(resp).await?;
        Ok(())
    })
    .await
}

// ---------------------------------------------------------------------------
// Message editing
// ---------------------------------------------------------------------------

/// Edit an existing message in a channel.
pub async fn edit_message(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    message_id: &str,
    new_content: &str,
) -> Result<()> {
    edit_message_with_url(client, bot_token, channel_id, message_id, new_content, BASE_URL).await
}

pub async fn edit_message_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel_id: &str,
    message_id: &str,
    new_content: &str,
    base_url: &str,
) -> Result<()> {
    with_retry(|| async {
        let url = format!("{base_url}/channels/{channel_id}/messages/{message_id}");
        let body = serde_json::json!({
            "content": new_content,
            "components": [],
        });

        let resp = client
            .patch(&url)
            .header("Authorization", auth_header(bot_token))
            .json(&body)
            .send()
            .await
            .map_err(|e| DiscordApiError::Transient(e.to_string()))?;

        classify_response(resp).await?;
        Ok(())
    })
    .await
}
