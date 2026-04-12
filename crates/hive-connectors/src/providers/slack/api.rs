use anyhow::{bail, Result};
use serde::Deserialize;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct AuthTestResponse {
    pub ok: bool,
    pub team: Option<String>,
    pub team_id: Option<String>,
    pub user: Option<String>,
    pub user_id: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SlackChannel {
    pub id: String,
    pub name: Option<String>,
    pub is_channel: Option<bool>,
    pub is_im: Option<bool>,
    pub is_member: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct ChatPostResponse {
    pub ok: bool,
    pub ts: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionsOpenResponse {
    pub ok: bool,
    pub url: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConversationsListResponse {
    ok: bool,
    channels: Option<Vec<SlackChannel>>,
    error: Option<String>,
    response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
struct ResponseMetadata {
    next_cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// API helpers
// ---------------------------------------------------------------------------

const BASE_URL: &str = "https://slack.com/api";

/// The default Slack API base URL.
pub fn default_base_url() -> &'static str {
    BASE_URL
}

/// Verify the bot token and return team/user information.
pub async fn auth_test(client: &reqwest::Client, bot_token: &str) -> Result<AuthTestResponse> {
    auth_test_with_url(client, bot_token, BASE_URL).await
}

/// Verify the bot token using a custom base URL (for testing).
pub async fn auth_test_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    base_url: &str,
) -> Result<AuthTestResponse> {
    let resp: AuthTestResponse = client
        .post(format!("{base_url}/auth.test"))
        .bearer_auth(bot_token)
        .header("Content-Type", "application/json")
        .send()
        .await?
        .json()
        .await?;

    if !resp.ok {
        bail!("Slack auth.test failed: {}", resp.error.as_deref().unwrap_or("unknown error"));
    }
    Ok(resp)
}

/// List conversations (channels, DMs) visible to the bot.
pub async fn conversations_list(
    client: &reqwest::Client,
    bot_token: &str,
) -> Result<Vec<SlackChannel>> {
    conversations_list_with_url(client, bot_token, BASE_URL).await
}

/// List conversations using a custom base URL (for testing).
pub async fn conversations_list_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    base_url: &str,
) -> Result<Vec<SlackChannel>> {
    let mut all_channels = Vec::new();
    let mut cursor: Option<String> = None;

    loop {
        let mut form = vec![("types", "public_channel,private_channel,im".to_string())];
        if let Some(ref c) = cursor {
            if !c.is_empty() {
                form.push(("cursor", c.clone()));
            }
        }

        let resp: ConversationsListResponse = client
            .post(format!("{base_url}/conversations.list"))
            .bearer_auth(bot_token)
            .form(&form)
            .send()
            .await?
            .json()
            .await?;

        if !resp.ok {
            bail!(
                "Slack conversations.list failed: {}",
                resp.error.as_deref().unwrap_or("unknown error")
            );
        }

        if let Some(channels) = resp.channels {
            all_channels.extend(channels);
        }

        match resp.response_metadata.and_then(|m| m.next_cursor).filter(|c| !c.is_empty()) {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    Ok(all_channels)
}

/// Send a message to a Slack channel and return the message timestamp (ts).
pub async fn chat_post_message(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
) -> Result<String> {
    chat_post_message_with_url(client, bot_token, channel, text, BASE_URL).await
}

/// Send a message using a custom base URL (for testing).
pub async fn chat_post_message_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
    base_url: &str,
) -> Result<String> {
    chat_post_message_rich_with_url(client, bot_token, channel, text, None, base_url).await
}

/// Send a message with optional Block Kit blocks.
pub async fn chat_post_message_rich(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
    blocks: Option<serde_json::Value>,
) -> Result<String> {
    chat_post_message_rich_with_url(client, bot_token, channel, text, blocks, BASE_URL).await
}

/// Send a message with optional Block Kit blocks using a custom base URL.
pub async fn chat_post_message_rich_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    text: &str,
    blocks: Option<serde_json::Value>,
    base_url: &str,
) -> Result<String> {
    let mut body = serde_json::json!({
        "channel": channel,
        "text": text,
    });

    if let Some(blks) = blocks {
        body["blocks"] = blks;
    }

    let resp: ChatPostResponse = client
        .post(format!("{base_url}/chat.postMessage"))
        .bearer_auth(bot_token)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if !resp.ok {
        bail!(
            "Slack chat.postMessage failed: {}",
            resp.error.as_deref().unwrap_or("unknown error")
        );
    }

    resp.ts.ok_or_else(|| anyhow::anyhow!("Slack chat.postMessage succeeded but returned no ts"))
}

/// Upload a file to a Slack channel using the files.upload API.
pub async fn files_upload(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    filename: &str,
    content: &[u8],
    initial_comment: Option<&str>,
) -> Result<()> {
    files_upload_with_url(client, bot_token, channel, filename, content, initial_comment, BASE_URL)
        .await
}

/// Upload a file using a custom base URL (for testing).
pub async fn files_upload_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    filename: &str,
    content: &[u8],
    initial_comment: Option<&str>,
    base_url: &str,
) -> Result<()> {
    let file_part =
        reqwest::multipart::Part::bytes(content.to_vec()).file_name(filename.to_string());

    let mut form = reqwest::multipart::Form::new()
        .text("channels", channel.to_string())
        .text("filename", filename.to_string())
        .part("file", file_part);

    if let Some(comment) = initial_comment {
        form = form.text("initial_comment", comment.to_string());
    }

    let resp: serde_json::Value = client
        .post(format!("{base_url}/files.upload"))
        .bearer_auth(bot_token)
        .multipart(form)
        .send()
        .await?
        .json()
        .await?;

    if resp["ok"].as_bool() != Some(true) {
        let error = resp["error"].as_str().unwrap_or("unknown error");
        bail!("Slack files.upload failed: {error}");
    }

    Ok(())
}

/// Open a Socket Mode WebSocket connection and return the WSS URL.
///
/// Uses the app-level token (xapp-…), **not** the bot token.
pub async fn connections_open(client: &reqwest::Client, app_token: &str) -> Result<String> {
    connections_open_with_url(client, app_token, BASE_URL).await
}

/// Open Socket Mode connection using a custom base URL (for testing).
pub async fn connections_open_with_url(
    client: &reqwest::Client,
    app_token: &str,
    base_url: &str,
) -> Result<String> {
    let resp: ConnectionsOpenResponse = client
        .post(format!("{base_url}/apps.connections.open"))
        .bearer_auth(app_token)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .send()
        .await?
        .json()
        .await?;

    if !resp.ok {
        bail!(
            "Slack apps.connections.open failed: {}",
            resp.error.as_deref().unwrap_or("unknown error")
        );
    }

    resp.url.ok_or_else(|| anyhow::anyhow!("Slack connections.open succeeded but returned no URL"))
}

// ---------------------------------------------------------------------------
// chat.update — edit an existing message
// ---------------------------------------------------------------------------

/// Update an existing Slack message.
pub async fn chat_update(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    ts: &str,
    text: &str,
) -> Result<()> {
    chat_update_with_url(client, bot_token, channel, ts, text, BASE_URL).await
}

pub async fn chat_update_with_url(
    client: &reqwest::Client,
    bot_token: &str,
    channel: &str,
    ts: &str,
    text: &str,
    base_url: &str,
) -> Result<()> {
    let body = serde_json::json!({
        "channel": channel,
        "ts": ts,
        "text": text,
        "blocks": [],
    });

    let resp: ChatPostResponse = client
        .post(format!("{base_url}/chat.update"))
        .bearer_auth(bot_token)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    if !resp.ok {
        bail!("Slack chat.update failed: {}", resp.error.as_deref().unwrap_or("unknown error"));
    }

    Ok(())
}
