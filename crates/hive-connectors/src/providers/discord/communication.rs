use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::{mpsc, Notify};
use tracing::{debug, info, warn};

use crate::services::communication::{
    CommAttachment, CommunicationService, InboundMessage, RichMessageBody,
};

use super::api;
use super::gateway::{self, GatewayMessage};

// ---------------------------------------------------------------------------
// Lazy gateway state — initialised on first use
// ---------------------------------------------------------------------------

struct GatewayState {
    rx: mpsc::Receiver<GatewayMessage>,
    _handle: tokio::task::JoinHandle<()>,
}

// ---------------------------------------------------------------------------
// DiscordCommunication
// ---------------------------------------------------------------------------

pub struct DiscordCommunication {
    connector_id: String,
    bot_token: String,
    allowed_guild_ids: Vec<String>,
    listen_channel_ids: Vec<String>,
    default_send_channel_id: Option<String>,
    http_client: reqwest::Client,
    /// Gateway state is lazily initialised on the first async call.
    state: tokio::sync::Mutex<Option<GatewayState>>,
    notify: Arc<Notify>,
}

impl DiscordCommunication {
    /// Create a communication service synchronously. The WebSocket gateway is
    /// started lazily on the first async operation.
    pub fn new(
        connector_id: &str,
        bot_token: String,
        allowed_guild_ids: Vec<String>,
        listen_channel_ids: Vec<String>,
        default_send_channel_id: Option<String>,
    ) -> Self {
        Self {
            connector_id: connector_id.to_string(),
            bot_token,
            allowed_guild_ids,
            listen_channel_ids,
            default_send_channel_id,
            http_client: reqwest::Client::new(),
            state: tokio::sync::Mutex::new(None),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Ensure the gateway is running; start it if not yet initialised.
    async fn ensure_gateway(&self) -> Result<()> {
        let mut guard = self.state.lock().await;
        if guard.is_some() {
            return Ok(());
        }

        let (handle, rx) = gateway::start_gateway(
            self.bot_token.clone(),
            self.allowed_guild_ids.clone(),
            self.listen_channel_ids.clone(),
            Arc::clone(&self.notify),
        )
        .await
        .context("failed to start Discord gateway")?;

        *guard = Some(GatewayState { rx, _handle: handle });
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// CommunicationService impl
// ---------------------------------------------------------------------------

#[async_trait]
impl CommunicationService for DiscordCommunication {
    fn name(&self) -> &str {
        "Discord Bot"
    }

    async fn test_connection(&self) -> Result<()> {
        let guilds = api::list_guilds(&self.http_client, &self.bot_token)
            .await
            .context("Discord test_connection: failed to list guilds")?;
        info!(guild_count = guilds.len(), "Discord connection verified");

        // Also kick off the gateway so it's ready for messages.
        self.ensure_gateway().await?;
        Ok(())
    }

    async fn send(
        &self,
        to: &[String],
        _subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let target_channel = if to.is_empty() || to[0].is_empty() {
            let default = self
                .default_send_channel_id
                .as_deref()
                .context("no target channel and no default_send_channel_id configured")?;
            warn!(
                default_channel = %default,
                "Discord send: no target channel specified in `to`, falling back to default_send_channel_id"
            );
            default
        } else {
            &to[0]
        };

        let message_id = if attachments.is_empty() {
            api::send_message(&self.http_client, &self.bot_token, target_channel, body)
                .await
                .with_context(|| format!("failed to send message to channel {target_channel}"))?
        } else {
            api::send_message_with_attachments(
                &self.http_client,
                &self.bot_token,
                target_channel,
                body,
                attachments,
            )
            .await
            .with_context(|| {
                format!("failed to send message with attachments to channel {target_channel}")
            })?
        };

        debug!(%message_id, %target_channel, "Discord message sent");
        Ok(message_id)
    }

    async fn send_rich(
        &self,
        to: &[String],
        _subject: Option<&str>,
        fallback_text: &str,
        rich_body: Option<RichMessageBody>,
        attachments: &[CommAttachment],
    ) -> Result<String> {
        let target_channel = if to.is_empty() || to[0].is_empty() {
            let default = self
                .default_send_channel_id
                .as_deref()
                .context("no target channel and no default_send_channel_id configured")?;
            default
        } else {
            &to[0]
        };

        // If there are attachments, use the attachment path (components not supported with attachments)
        if !attachments.is_empty() {
            return api::send_message_with_attachments(
                &self.http_client,
                &self.bot_token,
                target_channel,
                fallback_text,
                attachments,
            )
            .await
            .with_context(|| {
                format!("failed to send rich message with attachments to channel {target_channel}")
            });
        }

        let components = match rich_body {
            Some(RichMessageBody::DiscordComponents(c)) => Some(c),
            _ => None,
        };

        let message_id = api::send_message_rich(
            &self.http_client,
            &self.bot_token,
            target_channel,
            fallback_text,
            components,
        )
        .await
        .with_context(|| format!("failed to send rich message to channel {target_channel}"))?;

        debug!(%message_id, %target_channel, "Discord rich message sent");
        Ok(message_id)
    }

    async fn fetch_new(&self, limit: usize) -> Result<Vec<InboundMessage>> {
        self.ensure_gateway().await?;

        let mut guard = self.state.lock().await;
        let state = guard.as_mut().context("gateway not initialised")?;
        let mut messages = Vec::new();

        for _ in 0..limit {
            match state.rx.try_recv() {
                Ok(gm) => messages.push(gateway_to_inbound(gm, &self.connector_id)),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    warn!("Discord gateway channel disconnected");
                    break;
                }
            }
        }

        Ok(messages)
    }

    async fn mark_seen(&self, _message_id: &str) -> Result<()> {
        // Discord has no read-receipt / mark-as-read API for bots.
        Ok(())
    }

    fn supports_idle(&self) -> bool {
        true
    }

    async fn wait_for_changes(&self, timeout: Duration) -> Result<bool> {
        self.ensure_gateway().await?;

        match tokio::time::timeout(timeout, self.notify.notified()).await {
            Ok(()) => Ok(true),
            Err(_elapsed) => Ok(false),
        }
    }

    async fn edit_message(&self, channel_id: &str, message_id: &str, new_text: &str) -> Result<()> {
        api::edit_message(&self.http_client, &self.bot_token, channel_id, message_id, new_text)
            .await
    }

    async fn acknowledge_interaction(
        &self,
        interaction_id: &str,
        interaction_token: &str,
        update_content: &str,
    ) -> Result<()> {
        api::acknowledge_interaction(
            &self.http_client,
            interaction_id,
            interaction_token,
            update_content,
        )
        .await
    }

    async fn list_channels(&self) -> Result<Vec<crate::services::communication::ChannelInfo>> {
        use crate::services::communication::ChannelInfo;

        let mut result = Vec::new();
        let guilds = api::list_guilds(&self.http_client, &self.bot_token).await?;

        for guild in &guilds {
            if !self.allowed_guild_ids.is_empty() && !self.allowed_guild_ids.contains(&guild.id) {
                continue;
            }
            let channels =
                api::list_guild_channels(&self.http_client, &self.bot_token, &guild.id).await?;
            for ch in channels {
                // Type 0 = text, 2 = voice, 4 = category, 5 = announcement, 13 = stage, 15 = forum
                let ch_type = match ch.type_ {
                    0 => "text",
                    2 => "voice",
                    4 => "category",
                    5 => "announcement",
                    13 => "stage",
                    15 => "forum",
                    _ => "other",
                };
                result.push(ChannelInfo {
                    id: ch.id,
                    name: ch.name.unwrap_or_default(),
                    channel_type: Some(ch_type.to_string()),
                    group_name: Some(guild.name.clone()),
                });
            }
        }

        Ok(result)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn gateway_to_inbound(gm: GatewayMessage, connector_id: &str) -> InboundMessage {
    let from = format!("discord:{}#{}", gm.author_name, gm.author_id);

    let mut metadata = HashMap::new();
    metadata.insert("channel_id".to_string(), gm.channel_id.clone());
    metadata.insert("message_id".to_string(), gm.message_id.clone());
    if let Some(ref gid) = gm.guild_id {
        metadata.insert("guild_id".to_string(), gid.clone());
    }
    if let Some(ref ref_id) = gm.referenced_message_id {
        metadata.insert("referenced_message_id".to_string(), ref_id.clone());
    }
    if let Some(ref interaction) = gm.interaction {
        metadata.insert("interaction_custom_id".to_string(), interaction.custom_id.clone());
        metadata.insert("interaction_id".to_string(), interaction.interaction_id.clone());
        metadata.insert("interaction_token".to_string(), interaction.interaction_token.clone());
    }

    // Parse ISO-8601 timestamp to millis; fall back to current time.
    let timestamp_ms = chrono_parse_ms(&gm.timestamp).unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    });

    InboundMessage {
        external_id: format!("{connector_id}:{}", gm.message_id),
        from,
        to: vec![gm.channel_id],
        subject: None,
        body: gm.content,
        timestamp_ms,
        metadata,
        attachments: Vec::new(),
    }
}

/// Best-effort parse of an ISO-8601 timestamp string to epoch millis.
fn chrono_parse_ms(s: &str) -> Option<u128> {
    // Discord timestamps look like "2024-01-15T12:34:56.789000+00:00".
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return None;
    }

    let date_parts: Vec<u32> = parts[0].split('-').filter_map(|p| p.parse().ok()).collect();
    if date_parts.len() != 3 {
        return None;
    }
    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);

    // Time part: strip timezone suffix
    let time_str = parts[1].trim_end_matches('Z').split('+').next()?.split('-').next()?;
    let time_main: Vec<&str> = time_str.split(':').collect();
    if time_main.len() < 3 {
        return None;
    }
    let hour: u64 = time_main[0].parse().ok()?;
    let minute: u64 = time_main[1].parse().ok()?;
    // Seconds may have fractional part
    let sec_parts: Vec<&str> = time_main[2].splitn(2, '.').collect();
    let second: u64 = sec_parts[0].parse().ok()?;
    let millis_frac: u64 = if sec_parts.len() == 2 {
        let frac = sec_parts[1];
        let padded = format!("{:0<3}", &frac[..frac.len().min(3)]);
        padded.parse().unwrap_or(0)
    } else {
        0
    };

    // Convert to epoch millis (simplified — ignores leap seconds, timezone offset).
    fn days_from_civil(y: u32, m: u32, d: u32) -> i64 {
        let y = y as i64;
        let m = m as i64;
        let d = d as i64;
        let y = if m <= 2 { y - 1 } else { y };
        let era = if y >= 0 { y } else { y - 399 } / 400;
        let yoe = (y - era * 400) as u64;
        let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy as u64;
        era * 146097 + doe as i64 - 719468
    }

    let days = days_from_civil(year, month, day);
    let total_secs = days as u64 * 86400 + hour * 3600 + minute * 60 + second;
    Some(total_secs as u128 * 1000 + millis_frac as u128)
}
