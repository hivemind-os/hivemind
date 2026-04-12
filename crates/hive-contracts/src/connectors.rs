use hive_classification::DataClass;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// `MessageDirection` is defined in `crate::comms` and re-exported from the
// crate root.  Types in this module that reference it use the full path
// `crate::comms::MessageDirection` so there is no glob-reexport ambiguity.

// ---------------------------------------------------------------------------
// Connector provider
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConnectorProvider {
    /// Microsoft 365 / Outlook — uses Graph API exclusively.
    Microsoft,
    /// Gmail — Google OAuth.
    Gmail,
    /// Generic IMAP/SMTP with password or custom OAuth.
    #[serde(alias = "email")]
    Imap,
    Slack,
    Discord,
    /// Coinbase — OAuth2 for crypto trading via Advanced Trade API.
    Coinbase,
    /// Planned.
    Telegram,
    /// Planned.
    WhatsApp,
    /// Apple — local macOS Calendar & Contacts via native frameworks (macOS only).
    Apple,
}

impl ConnectorProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Microsoft => "microsoft",
            Self::Gmail => "gmail",
            Self::Imap => "imap",
            Self::Slack => "slack",
            Self::Discord => "discord",
            Self::Coinbase => "coinbase",
            Self::Telegram => "telegram",
            Self::WhatsApp => "whatsapp",
            Self::Apple => "apple",
        }
    }

    /// Returns the set of service types this provider can supply.
    pub fn available_services(&self) -> &'static [ServiceType] {
        match self {
            Self::Microsoft => &[
                ServiceType::Communication,
                ServiceType::Calendar,
                ServiceType::Drive,
                ServiceType::Contacts,
            ],
            Self::Gmail => &[
                ServiceType::Communication,
                ServiceType::Calendar,
                ServiceType::Drive,
                ServiceType::Contacts,
            ],
            Self::Imap | Self::Slack | Self::Discord | Self::Telegram | Self::WhatsApp => {
                &[ServiceType::Communication]
            }
            // Coinbase exposes a custom "trading" DynService, not a built-in archetype.
            Self::Coinbase => &[],
            Self::Apple => &[ServiceType::Calendar, ServiceType::Contacts],
        }
    }
}

impl std::fmt::Display for ConnectorProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Service type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceType {
    Communication,
    Calendar,
    Drive,
    Contacts,
    /// Extension point for custom service types not in the built-in set.
    #[serde(untagged)]
    Other(String),
}

impl ServiceType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Communication => "communication",
            Self::Calendar => "calendar",
            Self::Drive => "drive",
            Self::Contacts => "contacts",
            Self::Other(s) => s.as_str(),
        }
    }

    /// Returns `true` for the four built-in service archetypes.
    pub fn is_standard(&self) -> bool {
        matches!(self, Self::Communication | Self::Calendar | Self::Drive | Self::Contacts)
    }
}

impl std::fmt::Display for ServiceType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// Connector status
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "state")]
#[derive(Default)]
pub enum ConnectorStatus {
    Connected,
    #[default]
    Disconnected,
    AuthExpired,
    Error {
        message: String,
    },
}

// ---------------------------------------------------------------------------
// Connector info (summary for UI / tool output)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorInfo {
    pub id: String,
    pub name: String,
    pub provider: ConnectorProvider,
    pub enabled: bool,
    pub status: ConnectorStatus,
    pub enabled_services: Vec<ServiceType>,
    /// Persona IDs allowed to use this connector (empty = no access).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_personas: Vec<String>,
}

// ---------------------------------------------------------------------------
// Resource rule
// ---------------------------------------------------------------------------

/// A glob-pattern-based rule for a destination / resource.
///
/// Generalised from the earlier `DestinationRule` — works for any service
/// type (communication addresses, calendar patterns, drive paths, etc.).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceRule {
    /// Glob pattern for destination/resource matching.
    pub pattern: String,
    /// Approval behaviour for this resource.
    pub approval: crate::tools::ToolApproval,
    /// Override classification for inbound data from this resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_class_override: Option<DataClass>,
    /// Override classification for outbound data to this resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_class_override: Option<DataClass>,
}

// ---------------------------------------------------------------------------
// Communication message (connector-based)
// ---------------------------------------------------------------------------

/// A communication message tied to a connector (replaces the channel-based
/// `comms::CommMessage` once migration is complete).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorMessage {
    pub id: String,
    pub connector_id: String,
    pub provider: ConnectorProvider,
    pub direction: crate::comms::MessageDirection,
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
// Resolved resource policy
// ---------------------------------------------------------------------------

/// The effective policy for a specific resource after resolving all rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedResourcePolicy {
    pub approval: crate::tools::ToolApproval,
    pub input_class: DataClass,
    pub output_class: DataClass,
}

// ---------------------------------------------------------------------------
// Calendar types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EventStatus {
    Tentative,
    Confirmed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AttendeeResponse {
    Accepted,
    Declined,
    Tentative,
    #[serde(rename = "none")]
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attendee {
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub response: AttendeeResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarEvent {
    pub id: String,
    pub connector_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// ISO 8601 datetime.
    pub start: String,
    /// ISO 8601 datetime.
    pub end: String,
    pub is_all_day: bool,
    /// IANA timezone of the calendar (e.g. "America/New_York").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<Attendee>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organizer: Option<String>,
    pub status: EventStatus,
    pub data_class: DataClass,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_link: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewCalendarEvent {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub start: String,
    pub end: String,
    pub is_all_day: bool,
    /// Optional IANA timezone (e.g. "America/New_York"). If omitted, the
    /// calendar's default timezone is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// Email addresses of attendees.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventUpdate {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<String>,
    /// Optional IANA timezone for start/end. If omitted, the calendar's
    /// default timezone is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

// ---------------------------------------------------------------------------
// Free/busy
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FreeBusyStatus {
    Free,
    Busy,
    Tentative,
    OutOfOffice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreeBusySlot {
    pub start: String,
    pub end: String,
    pub status: FreeBusyStatus,
}

// ---------------------------------------------------------------------------
// Drive types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveItem {
    pub id: String,
    pub connector_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub is_folder: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// ISO 8601 datetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
    pub data_class: DataClass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveFileContent {
    pub item: DriveItem,
    pub content: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareLink {
    pub url: String,
    pub shared_with: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<String>,
}

// ---------------------------------------------------------------------------
// Contacts
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    pub id: String,
    pub connector_id: String,
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub email_addresses: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub phone_numbers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub company: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_title: Option<String>,
    pub data_class: DataClass,
}

// ---------------------------------------------------------------------------
// Service audit entry
// ---------------------------------------------------------------------------

/// Unified audit entry covering all connector service types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAuditEntry {
    pub id: String,
    pub connector_id: String,
    pub provider: ConnectorProvider,
    pub service_type: ServiceType,
    /// Operation name, e.g. "send", "read", "list_events", "upload_file".
    pub operation: String,
    pub direction: Option<crate::comms::MessageDirection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_address: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Event ID, file ID, contact ID, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_id: Option<String>,
    /// Event title, file name, contact name, etc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_name: Option<String>,
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
    fn connector_provider_round_trip() {
        let cp = ConnectorProvider::Imap;
        let json = serde_json::to_string(&cp).unwrap();
        assert_eq!(json, r#""imap""#);
        let back: ConnectorProvider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cp);
    }

    #[test]
    fn connector_provider_email_alias() {
        // Legacy "email" should deserialize to Imap.
        let cp: ConnectorProvider = serde_json::from_str(r#""email""#).unwrap();
        assert_eq!(cp, ConnectorProvider::Imap);
    }

    #[test]
    fn service_type_round_trip() {
        let st = ServiceType::Calendar;
        let json = serde_json::to_string(&st).unwrap();
        assert_eq!(json, r#""calendar""#);
        let back: ServiceType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, st);
    }

    #[test]
    fn resource_rule_round_trip() {
        let rule = ResourceRule {
            pattern: "*@outlook.com".into(),
            approval: crate::tools::ToolApproval::Auto,
            input_class_override: Some(DataClass::Confidential),
            output_class_override: None,
        };
        let json = serde_json::to_string(&rule).unwrap();
        let back: ResourceRule = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rule);
    }

    #[test]
    fn connector_status_variants() {
        let connected: ConnectorStatus = serde_json::from_str(r#"{"state":"connected"}"#).unwrap();
        assert_eq!(connected, ConnectorStatus::Connected);

        let err = ConnectorStatus::Error { message: "timeout".into() };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("timeout"));
    }

    #[test]
    fn available_services_microsoft() {
        let services = ConnectorProvider::Microsoft.available_services();
        assert_eq!(services.len(), 4);
        assert!(services.contains(&ServiceType::Communication));
        assert!(services.contains(&ServiceType::Calendar));
        assert!(services.contains(&ServiceType::Drive));
        assert!(services.contains(&ServiceType::Contacts));
    }

    #[test]
    fn available_services_gmail() {
        let services = ConnectorProvider::Gmail.available_services();
        assert_eq!(services.len(), 4);
        assert_eq!(services[0], ServiceType::Communication);
        assert_eq!(services[1], ServiceType::Calendar);
        assert_eq!(services[2], ServiceType::Drive);
        assert_eq!(services[3], ServiceType::Contacts);
    }
}
