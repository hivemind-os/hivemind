use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::services::communication::{
    CommAttachment, CommunicationService, InboundMessage, RichMessageBody,
};

use super::api;
use super::socket_mode::{self, SocketModeMessage};

// ---------------------------------------------------------------------------
// Inner state (deferred → connected)
// ---------------------------------------------------------------------------

struct ConnectedState {
    rx: mpsc::Receiver<SocketModeMessage>,
    notify: Arc<tokio::sync::Notify>,
    _socket_handle: tokio::task::JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// SlackCommunication
// ---------------------------------------------------------------------------

pub struct SlackCommunication {
    bot_token: String,
    app_token: String,
    listen_channel_ids: Vec<String>,
    default_send_channel_id: Option<String>,
    http_client: reqwest::Client,
    /// `None` until the Socket Mode connection is established.
    inner: tokio::sync::Mutex<Option<ConnectedState>>,
}

impl SlackCommunication {
    /// Create in *deferred* mode — no WebSocket yet.
    ///
    /// The actual Socket Mode connection is established lazily on the first
    /// call to `fetch_new` or `wait_for_changes` (or explicitly via
    /// `test_connection`).
    pub fn new_deferred(
        bot_token: String,
        app_token: String,
        listen_channel_ids: Vec<String>,
        default_send_channel_id: Option<String>,
    ) -> Self {
        Self {
            bot_token,
            app_token,
            listen_channel_ids,
            default_send_channel_id,
            http_client: reqwest::Client::new(),
            inner: tokio::sync::Mutex::new(None),
        }
    }

    /// Ensure the Socket Mode connection is established, connecting if needed.
    async fn ensure_connected(&self) -> Result<()> {
        let mut guard = self.inner.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let notify = Arc::new(tokio::sync::Notify::new());
        let (handle, rx) = socket_mode::start_socket_mode(
            self.app_token.clone(),
            self.bot_token.clone(),
            self.listen_channel_ids.clone(),
            Arc::clone(&notify),
        )
        .await
        .context("failed to start Slack Socket Mode")?;

        *guard = Some(ConnectedState { rx, notify, _socket_handle: handle });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CommunicationService impl
// ---------------------------------------------------------------------------

#[async_trait]
impl CommunicationService for SlackCommunication {
    fn name(&self) -> &str {
        "Slack Bot"
    }

    async fn test_connection(&self) -> Result<()> {
        let resp = api::auth_test(&self.http_client, &self.bot_token).await?;
        info!(
            "Slack connection OK — team={}, user={}",
            resp.team.as_deref().unwrap_or("?"),
            resp.user.as_deref().unwrap_or("?"),
        );
        Ok(())
    }

    async fn send(
        &self,
        to: &[String],
        _subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let channel = if to.is_empty() || to[0].is_empty() {
            self.default_send_channel_id.as_deref().unwrap_or_default()
        } else {
            &to[0]
        };

        // Upload any attachments first, then post the message
        if !attachments.is_empty() {
            for att in attachments {
                api::files_upload(
                    &self.http_client,
                    &self.bot_token,
                    channel,
                    &att.filename,
                    &att.data,
                    if body.is_empty() { None } else { Some(body) },
                )
                .await
                .with_context(|| {
                    format!("failed to upload file '{}' to Slack channel {channel}", att.filename)
                })?;
            }
            // If we uploaded files with a comment, we're done
            if !body.is_empty() {
                return Ok("uploaded-with-comment".to_string());
            }
        }

        let ts = api::chat_post_message(&self.http_client, &self.bot_token, channel, body)
            .await
            .with_context(|| format!("failed to post message to Slack channel {channel}"))?;

        debug!("Slack: sent message to {channel}, ts={ts}");
        Ok(ts)
    }

    async fn send_rich(
        &self,
        to: &[String],
        _subject: Option<&str>,
        fallback_text: &str,
        rich_body: Option<RichMessageBody>,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let channel = if to.is_empty() || to[0].is_empty() {
            self.default_send_channel_id.as_deref().unwrap_or_default()
        } else {
            &to[0]
        };

        // Upload attachments first (same as plain send)
        if !attachments.is_empty() {
            for att in attachments {
                api::files_upload(
                    &self.http_client,
                    &self.bot_token,
                    channel,
                    &att.filename,
                    &att.data,
                    if fallback_text.is_empty() { None } else { Some(fallback_text) },
                )
                .await
                .with_context(|| {
                    format!("failed to upload file '{}' to Slack channel {channel}", att.filename)
                })?;
            }
            if !fallback_text.is_empty() {
                return Ok("uploaded-with-comment".to_string());
            }
        }

        let blocks = match rich_body {
            Some(RichMessageBody::SlackBlocks(b)) => Some(b),
            _ => None,
        };

        let ts = api::chat_post_message_rich(
            &self.http_client,
            &self.bot_token,
            channel,
            fallback_text,
            blocks,
        )
        .await
        .with_context(|| format!("failed to post rich message to Slack channel {channel}"))?;

        debug!("Slack: sent rich message to {channel}, ts={ts}");
        Ok(ts)
    }

    async fn fetch_new(&self, limit: usize) -> Result<Vec<InboundMessage>> {
        self.ensure_connected().await?;

        let mut guard = self.inner.lock().await;
        let state = guard.as_mut().expect("connected after ensure_connected");
        let mut messages = Vec::new();

        for _ in 0..limit {
            match state.rx.try_recv() {
                Ok(sm) => {
                    let from = match &sm.user_name {
                        Some(name) => format!("slack:{name}"),
                        None => format!("slack:{}", sm.user_id),
                    };

                    let mut metadata = HashMap::new();
                    metadata.insert("ts".to_string(), sm.ts.clone());
                    metadata.insert("channel_id".to_string(), sm.channel_id.clone());
                    metadata.insert("user_id".to_string(), sm.user_id.clone());
                    if let Some(ref tts) = sm.thread_ts {
                        metadata.insert("thread_ts".to_string(), tts.clone());
                    }
                    if let Some(ref interaction) = sm.interaction {
                        metadata.insert(
                            "interaction_action_id".to_string(),
                            interaction.action_id.clone(),
                        );
                        metadata.insert("interaction_value".to_string(), interaction.value.clone());
                        if let Some(ref url) = interaction.response_url {
                            metadata.insert("response_url".to_string(), url.clone());
                        }
                    }

                    let timestamp_ms = parse_slack_ts_millis(&sm.ts);

                    messages.push(InboundMessage {
                        external_id: sm.ts,
                        from,
                        to: vec![sm.channel_id],
                        subject: None,
                        body: sm.text,
                        timestamp_ms,
                        metadata,
                        attachments: Vec::new(),
                    });
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("Slack Socket Mode channel disconnected");
                    break;
                }
            }
        }

        Ok(messages)
    }

    async fn mark_seen(&self, _message_id: &str) -> Result<()> {
        // Slack doesn't require explicit mark-as-read for bots
        Ok(())
    }

    fn supports_idle(&self) -> bool {
        true
    }

    async fn wait_for_changes(&self, timeout: Duration) -> Result<bool> {
        self.ensure_connected().await?;

        let notify = {
            let guard = self.inner.lock().await;
            let state = guard.as_ref().expect("connected after ensure_connected");
            Arc::clone(&state.notify)
        };

        match tokio::time::timeout(timeout, notify.notified()).await {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    async fn edit_message(&self, channel_id: &str, message_id: &str, new_text: &str) -> Result<()> {
        api::chat_update(&self.http_client, &self.bot_token, channel_id, message_id, new_text).await
    }

    async fn list_channels(&self) -> Result<Vec<crate::services::communication::ChannelInfo>> {
        use crate::services::communication::ChannelInfo;

        let channels = api::conversations_list(&self.http_client, &self.bot_token).await?;

        Ok(channels
            .into_iter()
            .map(|ch| {
                let ch_type = if ch.is_im.unwrap_or(false) {
                    "dm"
                } else if ch.is_channel.unwrap_or(false) {
                    "channel"
                } else {
                    "group"
                };
                ChannelInfo {
                    id: ch.id,
                    name: ch.name.unwrap_or_default(),
                    channel_type: Some(ch_type.to_string()),
                    group_name: None,
                }
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse a Slack message timestamp ("1234567890.123456") into epoch milliseconds.
fn parse_slack_ts_millis(ts: &str) -> u128 {
    // Slack ts format: "{seconds}.{microseconds}"
    let parts: Vec<&str> = ts.splitn(2, '.').collect();
    let secs: u128 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let micros: u128 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    secs * 1000 + micros / 1000
}
