use async_trait::async_trait;
use std::collections::HashMap;
use std::time::Duration;

/// A file attachment on an outbound or inbound communication message.
#[derive(Debug, Clone)]
pub struct CommAttachment {
    /// Provider-specific attachment ID (set on inbound messages for lazy download).
    pub id: Option<String>,
    /// The filename (e.g. "report.pdf").
    pub filename: String,
    /// MIME type (e.g. "application/pdf").
    pub media_type: String,
    /// Raw binary content.
    pub data: Vec<u8>,
}

/// Provider-specific rich message body for interactive messages.
#[derive(Debug, Clone)]
pub enum RichMessageBody {
    /// Slack Block Kit JSON blocks.
    SlackBlocks(serde_json::Value),
    /// Discord message components (action rows with buttons).
    DiscordComponents(serde_json::Value),
    /// HTML body for email providers.
    Html(String),
}

/// Abstract interface for the Communication service on a connector.
///
/// Ported from the `ChannelConnector` trait in `hive-comms`.
/// Each provider that supports messaging implements this trait.
#[async_trait]
pub trait CommunicationService: Send + Sync {
    /// Human-readable name (e.g. "Microsoft 365 Mail").
    fn name(&self) -> &str;

    /// Test communication connectivity and authentication.
    async fn test_connection(&self) -> anyhow::Result<()>;

    /// Send a message with optional file attachments.  Returns the provider-specific message ID.
    async fn send(
        &self,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[CommAttachment],
    ) -> anyhow::Result<String>;

    /// Send a rich/interactive message. For Slack this includes Block Kit blocks,
    /// for Discord message components with buttons, and for email HTML bodies.
    /// Default falls back to plain text `send()`.
    async fn send_rich(
        &self,
        to: &[String],
        subject: Option<&str>,
        fallback_text: &str,
        _rich_body: Option<RichMessageBody>,
        attachments: &[CommAttachment],
    ) -> anyhow::Result<String> {
        self.send(to, subject, fallback_text, attachments).await
    }

    /// Fetch new/unread messages since the last check.
    async fn fetch_new(&self, limit: usize) -> anyhow::Result<Vec<InboundMessage>>;

    /// Mark a message as seen/read.
    async fn mark_seen(&self, message_id: &str) -> anyhow::Result<()>;

    /// Whether this connector supports push-based notification (e.g. IMAP IDLE).
    fn supports_idle(&self) -> bool {
        false
    }

    /// Block until the server signals new messages, or until `timeout` expires.
    ///
    /// Returns `true` if there are likely new messages to fetch, `false` on
    /// timeout.  Connectors that don't support IDLE return `false` immediately.
    async fn wait_for_changes(&self, timeout: Duration) -> anyhow::Result<bool> {
        let _ = timeout;
        Ok(false)
    }

    /// Edit an existing message (best-effort). Not all providers support this.
    /// `message_id` is the provider-specific ID (Slack ts, Discord message ID).
    /// `channel_id` is the provider-specific channel (Slack channel, Discord channel).
    async fn edit_message(
        &self,
        _channel_id: &str,
        _message_id: &str,
        _new_text: &str,
    ) -> anyhow::Result<()> {
        Ok(()) // No-op for providers that don't support editing (email).
    }

    /// Acknowledge a Discord-style interaction callback. No-op for other providers.
    async fn acknowledge_interaction(
        &self,
        _interaction_id: &str,
        _interaction_token: &str,
        _update_content: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    /// List the channels, rooms, or folders available on this connector.
    ///
    /// For Discord this returns guild channels, for Slack workspace channels,
    /// for email providers mailbox folders.  Default returns an empty list.
    async fn list_channels(&self) -> anyhow::Result<Vec<ChannelInfo>> {
        Ok(Vec::new())
    }

    /// Download a specific attachment by message and attachment ID.
    ///
    /// Returns the full attachment including its binary content.
    /// Default returns an error for providers that don't support lazy attachment download.
    async fn download_attachment(
        &self,
        _message_id: &str,
        _attachment_id: &str,
    ) -> anyhow::Result<CommAttachment> {
        anyhow::bail!("download_attachment is not supported by this provider")
    }
}

/// A raw inbound message from a connector, before classification.
#[derive(Debug, Clone)]
pub struct InboundMessage {
    /// Connector-specific message identifier.
    pub external_id: String,
    pub from: String,
    pub to: Vec<String>,
    pub subject: Option<String>,
    pub body: String,
    pub timestamp_ms: u128,
    /// Connector-specific metadata (e.g. IMAP UID, thread ID).
    pub metadata: HashMap<String, String>,
    /// File attachments on this message (may be empty).
    pub attachments: Vec<CommAttachment>,
}

/// A channel, room, or folder that the LLM can address messages to.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChannelInfo {
    /// Provider-specific channel/folder identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// E.g. "text", "voice", "dm", "folder", "group".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_type: Option<String>,
    /// Parent group name (Discord guild, Slack workspace, etc.).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
}
