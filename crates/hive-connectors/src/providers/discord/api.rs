use anyhow::{Context, Result};
use serde::Deserialize;
use tracing::warn;

use crate::services::communication::CommAttachment;

const BASE_URL: &str = "https://discord.com/api/v10";

/// The default Discord API base URL.
pub fn default_base_url() -> &'static str {
    BASE_URL
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
    let url = format!("{base_url}/users/@me/guilds");
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(bot_token))
        .send()
        .await
        .context("failed to send list_guilds request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, %body, "Discord list_guilds failed");
        anyhow::bail!("Discord API error {status}: {body}");
    }

    resp.json::<Vec<Guild>>().await.context("failed to parse list_guilds response")
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
    let url = format!("{base_url}/guilds/{guild_id}/channels");
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(bot_token))
        .send()
        .await
        .context("failed to send list_guild_channels request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, %body, guild_id, "Discord list_guild_channels failed");
        anyhow::bail!("Discord API error {status}: {body}");
    }

    resp.json::<Vec<Channel>>().await.context("failed to parse list_guild_channels response")
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
        .context("failed to send send_message request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, %body, channel_id, "Discord send_message failed");
        anyhow::bail!("Discord API error {status}: {body}");
    }

    let msg: MessageResponse =
        resp.json().await.context("failed to parse send_message response")?;
    Ok(msg.id)
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
    let url = format!("{base_url}/channels/{channel_id}/messages");

    // Build the JSON payload_json with attachment metadata
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

    let mut form = reqwest::multipart::Form::new()
        .text("payload_json", serde_json::to_string(&payload).context("serializing payload_json")?);

    for (i, att) in attachments.iter().enumerate() {
        let part = reqwest::multipart::Part::bytes(att.data.clone())
            .file_name(att.filename.clone())
            .mime_str(&att.media_type)
            .unwrap_or_else(|_| {
                reqwest::multipart::Part::bytes(att.data.clone()).file_name(att.filename.clone())
            });
        form = form.part(format!("files[{i}]"), part);
    }

    let resp = client
        .post(&url)
        .header("Authorization", auth_header(bot_token))
        .multipart(form)
        .send()
        .await
        .context("failed to send message with attachments")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, %body, channel_id, "Discord send_message_with_attachments failed");
        anyhow::bail!("Discord API error {status}: {body}");
    }

    let msg: MessageResponse =
        resp.json().await.context("failed to parse send_message response")?;
    Ok(msg.id)
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
    let url = format!("{base_url}/gateway/bot");
    let resp = client
        .get(&url)
        .header("Authorization", auth_header(bot_token))
        .send()
        .await
        .context("failed to send get_gateway_url request")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        warn!(%status, %body, "Discord get_gateway_url failed");
        anyhow::bail!("Discord API error {status}: {body}");
    }

    let gw: GatewayBotResponse =
        resp.json().await.context("failed to parse get_gateway_url response")?;
    Ok(gw.url)
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
    // Type 7 = UPDATE_MESSAGE — acknowledges and edits the original message.
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
        .context("failed to send interaction callback")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        warn!(%status, %text, "Discord interaction callback failed");
        anyhow::bail!("Discord interaction callback error {status}: {text}");
    }

    Ok(())
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
        .context("failed to edit Discord message")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        warn!(%status, %text, channel_id, message_id, "Discord edit_message failed");
        anyhow::bail!("Discord API error {status}: {text}");
    }

    Ok(())
}
