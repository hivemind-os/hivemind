use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use super::api;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

pub(crate) struct SocketModeMessage {
    pub channel_id: String,
    pub user_id: String,
    pub user_name: Option<String>,
    pub text: String,
    pub ts: String,
    pub thread_ts: Option<String>,
    /// If this message is from a block_actions interaction (button click).
    pub interaction: Option<SlackInteraction>,
}

/// A parsed Slack block_actions interaction (e.g. button clicks from AFK messages).
pub(crate) struct SlackInteraction {
    pub action_id: String,
    pub value: String,
    pub response_url: Option<String>,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Start the Slack Socket Mode connection as a background task.
///
/// Returns a join handle for the background task and a receiver for incoming
/// messages that matched the filter criteria.
pub(crate) async fn start_socket_mode(
    app_token: String,
    bot_token: String,
    listen_channel_ids: Vec<String>,
    notify: Arc<tokio::sync::Notify>,
) -> Result<(tokio::task::JoinHandle<()>, mpsc::Receiver<SocketModeMessage>)> {
    let (tx, rx) = mpsc::channel::<SocketModeMessage>(256);

    let http_client = reqwest::Client::new();

    // Obtain the initial WSS URL before spawning so that early failures
    // surface to the caller immediately.
    let initial_url = api::connections_open(&http_client, &app_token).await?;

    let handle = tokio::spawn(async move {
        run_loop(http_client, app_token, bot_token, listen_channel_ids, tx, notify, initial_url)
            .await;
    });

    Ok((handle, rx))
}

// ---------------------------------------------------------------------------
// Internal run loop
// ---------------------------------------------------------------------------

async fn run_loop(
    http_client: reqwest::Client,
    app_token: String,
    _bot_token: String,
    listen_channel_ids: Vec<String>,
    tx: mpsc::Sender<SocketModeMessage>,
    notify: Arc<tokio::sync::Notify>,
    initial_url: String,
) {
    let mut wss_url = initial_url;
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        if tx.is_closed() {
            info!("Slack Socket Mode: receiver dropped, exiting");
            return;
        }

        info!("Slack Socket Mode: connecting to WebSocket");
        let ws_stream = match tokio_tungstenite::connect_async(&wss_url).await {
            Ok((stream, _)) => {
                backoff = Duration::from_secs(1);
                stream
            }
            Err(e) => {
                warn!("Slack Socket Mode: WebSocket connect error: {e}");
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                // Get a fresh URL on reconnect
                match api::connections_open(&http_client, &app_token).await {
                    Ok(url) => wss_url = url,
                    Err(e) => {
                        error!("Slack Socket Mode: connections.open failed: {e}");
                    }
                }
                continue;
            }
        };

        let (mut sink, mut stream) = ws_stream.split();

        loop {
            if tx.is_closed() {
                info!("Slack Socket Mode: receiver dropped, exiting");
                return;
            }

            let msg = match stream.next().await {
                Some(Ok(msg)) => msg,
                Some(Err(e)) => {
                    warn!("Slack Socket Mode: WebSocket read error: {e}");
                    break; // reconnect
                }
                None => {
                    info!("Slack Socket Mode: WebSocket stream ended");
                    break; // reconnect
                }
            };

            let text = match msg {
                WsMessage::Text(t) => t,
                WsMessage::Ping(data) => {
                    if let Err(e) = sink.send(WsMessage::Pong(data)).await {
                        warn!("Slack Socket Mode: failed to send pong: {e}");
                        break;
                    }
                    continue;
                }
                WsMessage::Close(_) => {
                    info!("Slack Socket Mode: received close frame");
                    break;
                }
                _ => continue,
            };

            let envelope: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => {
                    warn!("Slack Socket Mode: failed to parse message JSON: {e}");
                    continue;
                }
            };

            let msg_type = envelope["type"].as_str().unwrap_or("");
            let envelope_id = envelope["envelope_id"].as_str().unwrap_or("");

            match msg_type {
                "hello" => {
                    info!("Slack Socket Mode: received hello");
                }
                "disconnect" => {
                    info!("Slack Socket Mode: received disconnect, reconnecting");
                    // Get a fresh URL
                    match api::connections_open(&http_client, &app_token).await {
                        Ok(url) => wss_url = url,
                        Err(e) => {
                            error!("Slack Socket Mode: connections.open failed on disconnect: {e}");
                        }
                    }
                    break;
                }
                "events_api" => {
                    // ACK immediately
                    if !envelope_id.is_empty() {
                        let ack = serde_json::json!({ "envelope_id": envelope_id });
                        if let Err(e) = sink.send(WsMessage::Text(ack.to_string().into())).await {
                            warn!("Slack Socket Mode: failed to send ACK: {e}");
                            break;
                        }
                    }

                    if let Some(parsed) = parse_event(&envelope, &listen_channel_ids) {
                        if tx.send(parsed).await.is_err() {
                            info!("Slack Socket Mode: receiver dropped, exiting");
                            return;
                        }
                        notify.notify_waiters();
                    }
                }
                "interactive" => {
                    // ACK immediately
                    if !envelope_id.is_empty() {
                        let ack = serde_json::json!({ "envelope_id": envelope_id });
                        if let Err(e) = sink.send(WsMessage::Text(ack.to_string().into())).await {
                            warn!("Slack Socket Mode: failed to send ACK: {e}");
                            break;
                        }
                    }

                    if let Some(parsed) = parse_block_actions(&envelope) {
                        debug!(action_id = %parsed.interaction.as_ref().map(|i| i.action_id.as_str()).unwrap_or(""), "Slack Socket Mode: received block_actions");
                        if tx.send(parsed).await.is_err() {
                            info!("Slack Socket Mode: receiver dropped, exiting");
                            return;
                        }
                        notify.notify_waiters();
                    }
                }
                other => {
                    debug!("Slack Socket Mode: ignoring envelope type: {other}");
                    // ACK unknown types to prevent retries
                    if !envelope_id.is_empty() {
                        let ack = serde_json::json!({ "envelope_id": envelope_id });
                        let _ = sink.send(WsMessage::Text(ack.to_string().into())).await;
                    }
                }
            }
        }

        // Reconnect with backoff
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);

        match api::connections_open(&http_client, &app_token).await {
            Ok(url) => {
                wss_url = url;
                backoff = Duration::from_secs(1);
            }
            Err(e) => {
                error!("Slack Socket Mode: connections.open failed during reconnect: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Event parsing
// ---------------------------------------------------------------------------

fn parse_event(
    envelope: &serde_json::Value,
    listen_channel_ids: &[String],
) -> Option<SocketModeMessage> {
    let event = envelope.get("payload")?.get("event")?;

    // Only handle "message" events
    if event.get("type")?.as_str()? != "message" {
        return None;
    }

    // Skip bot messages
    if event.get("bot_id").is_some() {
        debug!("Slack Socket Mode: skipping bot message");
        return None;
    }

    // Skip messages with subtype (edits, joins, topic changes, etc.)
    if event.get("subtype").is_some() {
        debug!("Slack Socket Mode: skipping message with subtype");
        return None;
    }

    let channel_id = event.get("channel")?.as_str()?.to_string();

    // Filter by channel whitelist
    if !listen_channel_ids.is_empty() && !listen_channel_ids.contains(&channel_id) {
        debug!("Slack Socket Mode: skipping message from non-listened channel {channel_id}");
        return None;
    }

    let user_id = event.get("user")?.as_str()?.to_string();
    let text = event.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let ts = event.get("ts")?.as_str()?.to_string();
    let thread_ts = event.get("thread_ts").and_then(|v| v.as_str()).map(|s| s.to_string());

    // user_name is not always present in Socket Mode events
    let user_name = event
        .get("user_profile")
        .and_then(|p| p.get("display_name"))
        .and_then(|v| v.as_str())
        .or_else(|| event.get("user_profile").and_then(|p| p.get("name")).and_then(|v| v.as_str()))
        .map(|s| s.to_string());

    Some(SocketModeMessage {
        channel_id,
        user_id,
        user_name,
        text,
        ts,
        thread_ts,
        interaction: None,
    })
}

/// Parse a Slack Socket Mode `interactive` envelope containing block_actions.
fn parse_block_actions(envelope: &serde_json::Value) -> Option<SocketModeMessage> {
    let payload = envelope.get("payload")?;

    // Only handle block_actions
    if payload.get("type")?.as_str()? != "block_actions" {
        return None;
    }

    let actions = payload.get("actions")?.as_array()?;
    let first_action = actions.first()?;

    let action_id = first_action.get("action_id")?.as_str()?.to_string();
    let value = first_action.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string();

    let user = payload.get("user")?;
    let user_id = user.get("id")?.as_str()?.to_string();
    let user_name = user.get("name").and_then(|v| v.as_str()).map(|s| s.to_string());

    let channel_id = payload
        .get("channel")
        .and_then(|c| c.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let message = payload.get("message");
    let ts = message.and_then(|m| m.get("ts")).and_then(|v| v.as_str()).unwrap_or("").to_string();

    let response_url = payload.get("response_url").and_then(|v| v.as_str()).map(|s| s.to_string());

    Some(SocketModeMessage {
        channel_id,
        user_id,
        user_name,
        text: String::new(),
        ts,
        thread_ts: None,
        interaction: Some(SlackInteraction { action_id, value, response_url }),
    })
}
