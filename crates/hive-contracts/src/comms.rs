use hive_classification::DataClass;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Channel types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChannelType {
    /// Microsoft 365 / Outlook — uses Graph API exclusively.
    Microsoft,
    /// Gmail — Google OAuth with IMAP/SMTP.
    Gmail,
    /// Generic IMAP/SMTP with password or custom OAuth.
    #[serde(alias = "email")]
    Imap,
    Slack,
    Discord,
    Telegram,
    WhatsApp,
}

impl ChannelType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Microsoft => "microsoft",
            Self::Gmail => "gmail",
            Self::Imap => "imap",
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::Telegram => "telegram",
            Self::WhatsApp => "whatsapp",
        }
    }
}

impl std::fmt::Display for ChannelType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Message direction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageDirection {
    Inbound,
    Outbound,
}

impl MessageDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbound => "inbound",
            Self::Outbound => "outbound",
        }
    }
}

impl std::fmt::Display for MessageDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Channel status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
#[derive(Default)]
pub enum ChannelStatus {
    Connected,
    #[default]
    Disconnected,
    AuthExpired,
    Error {
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Channel info (summary for tool output / UI)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub enabled: bool,
    pub status: ChannelStatus,
}

// ---------------------------------------------------------------------------
// Destination rule
// ---------------------------------------------------------------------------

/// A glob-pattern-based rule for a specific destination address.
///
/// Examples:
/// - `*@outlook.com` → auto-approve, outgoing = public
/// - `*@gmail.com` → ask, use channel defaults
/// - `boss@gmail.com` → deny
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DestinationRule {
    /// Glob pattern for destination address matching.
    pub pattern: String,
    /// Approval behaviour for this destination.
    pub approval: crate::tools::ToolApproval,
    /// Override classification for inbound data from this destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_class_override: Option<DataClass>,
    /// Override classification for outbound data to this destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_class_override: Option<DataClass>,
}

// ---------------------------------------------------------------------------
// Communication message
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommMessage {
    pub id: String,
    pub channel_id: String,
    pub channel_type: ChannelType,
    pub direction: MessageDirection,
    pub from: String,
    pub to: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body: String,
    pub timestamp_ms: u128,
    pub data_class: DataClass,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub metadata: HashMap<String, String>,
}

// ---------------------------------------------------------------------------
// Resolved destination policy
// ---------------------------------------------------------------------------

/// The effective policy for a specific destination after resolving all rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedDestinationPolicy {
    pub approval: crate::tools::ToolApproval,
    pub input_class: DataClass,
    pub output_class: DataClass,
}

// ---------------------------------------------------------------------------
// Communication audit entry (queryable form)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommAuditEntry {
    pub id: String,
    pub channel_id: String,
    pub channel_type: ChannelType,
    pub direction: MessageDirection,
    pub from_address: String,
    pub to_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    pub body_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_preview: Option<String>,
    pub data_class: DataClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_decision: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub timestamp_ms: u128,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_type_round_trip() {
        let ct = ChannelType::Imap;
        let json = serde_json::to_string(&ct).unwrap();
        assert_eq!(json, r#""imap""#);
        let back: ChannelType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ct);
    }

    #[test]
    fn channel_type_email_alias() {
        // Legacy "email" should deserialize to Imap
        let ct: ChannelType = serde_json::from_str(r#""email""#).unwrap();
        assert_eq!(ct, ChannelType::Imap);
    }

    #[test]
    fn destination_rule_round_trip() {
        let rule = DestinationRule {
            pattern: "*@outlook.com".into(),
            approval: crate::tools::ToolApproval::Auto,
            input_class_override: Some(DataClass::Confidential),
            output_class_override: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let back: DestinationRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rule);
    }

    #[test]
    fn channel_status_variants() {
        let connected: ChannelStatus = serde_json::from_str(r#"{"state":"connected"}"#).unwrap();
        assert_eq!(connected, ChannelStatus::Connected);

        let err = ChannelStatus::Error { message: "timeout".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("timeout"));
    }
}
