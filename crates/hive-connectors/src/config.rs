use hive_classification::DataClass;
use hive_contracts::connectors::{ConnectorProvider, ResourceRule};
use serde::{Deserialize, Serialize};
use tracing::warn;

// ---------------------------------------------------------------------------
// SMTP encryption mode
// ---------------------------------------------------------------------------

/// Controls the TLS strategy for outbound SMTP connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SmtpEncryption {
    /// STARTTLS — connect in plaintext, then upgrade (port 587).
    #[default]
    Starttls,
    /// Implicit TLS / SMTPS — TLS from the first byte (port 465).
    /// Required by Amazon WorkMail and some other providers.
    ImplicitTls,
}

// ---------------------------------------------------------------------------
// Top-level connector configuration (persisted as YAML)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorConfig {
    pub id: String,
    pub name: String,
    pub provider: ConnectorProvider,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub auth: AuthConfig,
    #[serde(default)]
    pub services: ServicesConfig,
    /// Persona IDs that are allowed to see and use this connector.
    /// Empty means no personas have access (user must explicitly grant).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_personas: Vec<String>,
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// Auth configuration (provider-specific)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AuthConfig {
    /// OAuth2 device-code / authorization-code flow (Microsoft, Gmail).
    #[serde(rename = "oauth2")]
    OAuth2 {
        #[serde(default)]
        client_id: String,
        #[serde(default, skip_serializing)]
        client_secret: Option<String>,
        #[serde(default, skip_serializing)]
        refresh_token: String,
        #[serde(default, skip_serializing)]
        access_token: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        token_url: Option<String>,
    },
    /// Generic IMAP/SMTP password auth.
    Password {
        username: String,
        #[serde(default, skip_serializing)]
        password: String,
        imap_host: String,
        #[serde(default = "default_imap_port")]
        imap_port: u16,
        smtp_host: String,
        #[serde(default = "default_smtp_port")]
        smtp_port: u16,
        #[serde(default)]
        smtp_encryption: SmtpEncryption,
    },
    /// Bot token auth (Discord, Slack).
    BotToken {
        #[serde(default, skip_serializing)]
        bot_token: String,
        /// Slack Socket Mode app token.
        #[serde(default, skip_serializing, skip_serializing_if = "Option::is_none")]
        app_token: Option<String>,
    },
    /// Coinbase CDP API Key auth (ES256 JWT signing).
    CdpApiKey {
        /// Key name, e.g. `organizations/{org_id}/apiKeys/{key_id}`.
        key_name: String,
        /// EC private key in PEM format.
        #[serde(default, skip_serializing)]
        private_key: String,
    },
    /// Local OS-level auth (e.g. macOS TCC permissions). No secrets needed.
    Local,
}

fn default_imap_port() -> u16 {
    993
}

fn default_smtp_port() -> u16 {
    587
}

// ---------------------------------------------------------------------------
// Per-service configuration
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServicesConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub communication: Option<CommunicationConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar: Option<CalendarConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drive: Option<DriveConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contacts: Option<ContactsConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trading: Option<TradingConfig>,
    /// Extension point: arbitrary custom service configurations keyed by
    /// service type (e.g. `"ticketing"`, `"crm"`).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub custom: std::collections::HashMap<String, GenericServiceConfig>,
}

/// Configuration for a custom (non-standard) connector service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenericServiceConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default_input_class: DataClass,
    #[serde(default)]
    pub default_output_class: DataClass,
    /// Provider-specific configuration (opaque JSON blob).
    #[serde(default)]
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunicationConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_address: Option<String>,
    #[serde(default = "default_folder")]
    pub folder: String,
    #[serde(default = "default_poll_interval_secs", skip_serializing_if = "Option::is_none")]
    pub poll_interval_secs: Option<u64>,
    #[serde(default)]
    pub default_input_class: DataClass,
    #[serde(default)]
    pub default_output_class: DataClass,
    #[serde(default)]
    pub destination_rules: Vec<ResourceRule>,
    // Discord/Slack specific
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_guild_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub listen_channel_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_send_channel_id: Option<String>,
}

fn default_folder() -> String {
    "INBOX".to_string()
}

fn default_poll_interval_secs() -> Option<u64> {
    Some(60)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default_class: DataClass,
    #[serde(default)]
    pub resource_rules: Vec<ResourceRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriveConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default_class: DataClass,
    #[serde(default)]
    pub resource_rules: Vec<ResourceRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContactsConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default_class: DataClass,
    #[serde(default)]
    pub resource_rules: Vec<ResourceRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradingConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub default_input_class: DataClass,
    #[serde(default)]
    pub default_output_class: DataClass,
    /// When `true`, the connector targets the Coinbase sandbox
    /// (`api-sandbox.coinbase.com`) instead of production.
    #[serde(default)]
    pub sandbox: bool,
}

// ---------------------------------------------------------------------------
// OAuth scope computation
// ---------------------------------------------------------------------------

impl ConnectorConfig {
    /// Compute the OAuth scopes required for the enabled services.
    ///
    /// Returns `None` for non-OAuth providers.
    pub fn required_oauth_scopes(&self) -> Option<Vec<&'static str>> {
        match self.provider {
            ConnectorProvider::Microsoft => {
                let mut scopes = vec!["offline_access", "User.Read"];
                if self.services.communication.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://graph.microsoft.com/Mail.Send");
                    scopes.push("https://graph.microsoft.com/Mail.ReadWrite");
                }
                if self.services.calendar.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://graph.microsoft.com/Calendars.ReadWrite");
                }
                if self.services.drive.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://graph.microsoft.com/Files.ReadWrite.All");
                }
                if self.services.contacts.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://graph.microsoft.com/Contacts.Read");
                }
                Some(scopes)
            }
            ConnectorProvider::Gmail => {
                let mut scopes = vec!["openid", "email", "profile"];
                if self.services.communication.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://www.googleapis.com/auth/gmail.modify");
                    scopes.push("https://www.googleapis.com/auth/gmail.send");
                }
                if self.services.calendar.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://www.googleapis.com/auth/calendar");
                }
                if self.services.drive.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://www.googleapis.com/auth/drive");
                }
                if self.services.contacts.as_ref().is_some_and(|c| c.enabled) {
                    scopes.push("https://www.googleapis.com/auth/contacts.readonly");
                }
                Some(scopes)
            }
            ConnectorProvider::Coinbase => None, // CDP API Key auth; no OAuth scopes.
            _ => None,
        }
    }

    /// Check if this connector has enough credentials to attempt a connection.
    pub fn has_credentials(&self) -> bool {
        match &self.auth {
            AuthConfig::OAuth2 { refresh_token, access_token, .. } => {
                !refresh_token.is_empty() || access_token.as_ref().is_some_and(|t| !t.is_empty())
            }
            AuthConfig::Password { password, .. } => !password.is_empty(),
            AuthConfig::BotToken { bot_token, .. } => !bot_token.is_empty(),
            AuthConfig::CdpApiKey { key_name, private_key } => {
                !key_name.is_empty() && !private_key.is_empty()
            }
            // Local connectors rely on OS permissions; always considered credentialed.
            AuthConfig::Local => true,
        }
    }

    /// Return the email from-address if configured on the communication service.
    pub fn email_from_address(&self) -> Option<&str> {
        self.services.communication.as_ref().and_then(|c| c.from_address.as_deref())
    }

    /// Return the poll interval if configured on the communication service.
    pub fn poll_interval_secs(&self) -> Option<u64> {
        self.services.communication.as_ref().and_then(|c| c.poll_interval_secs)
    }

    /// Persist secret fields to the OS keyring.
    ///
    /// Restore secrets from the OS keyring into this config's auth fields.
    /// Call this when you have a config from the UI that has empty secret fields
    /// (because secrets are `skip_serializing`) and need to populate them.
    pub fn restore_secrets(&mut self) {
        match &mut self.auth {
            AuthConfig::OAuth2 { client_secret, refresh_token, access_token, .. } => {
                if let Some(s) = crate::secrets::load(&self.id, "client_secret") {
                    *client_secret = Some(s);
                }
                if refresh_token.is_empty() {
                    if let Some(s) = crate::secrets::load(&self.id, "refresh_token") {
                        *refresh_token = s;
                    }
                }
                // Restore access_token from the in-memory cache if the
                // config field is empty.  Also handles the `Some("")`
                // edge case from frontends that send empty-string values.
                let needs_restore = match access_token.as_deref() {
                    None => true,
                    Some("") => true,
                    _ => false,
                };
                if needs_restore {
                    *access_token = crate::secrets::load(&self.id, "access_token");
                }
            }
            AuthConfig::Password { password, .. } => {
                if password.is_empty() {
                    if let Some(s) = crate::secrets::load(&self.id, "password") {
                        *password = s;
                    }
                }
            }
            AuthConfig::BotToken { bot_token, app_token, .. } => {
                if bot_token.is_empty() {
                    if let Some(s) = crate::secrets::load(&self.id, "bot_token") {
                        *bot_token = s;
                    }
                }
                if app_token.is_none() {
                    *app_token = crate::secrets::load(&self.id, "app_token");
                }
            }
            AuthConfig::CdpApiKey { private_key, key_name } => {
                if private_key.is_empty() {
                    if let Some(s) = crate::secrets::load(&self.id, "private_key") {
                        *private_key = s;
                    } else {
                        warn!(
                            connector_id = %self.id,
                            key_name = %key_name,
                            "no private_key found in keyring for Coinbase connector"
                        );
                    }
                }
            }
            AuthConfig::Local => {}
        }
    }

    /// Call this before serializing to YAML so secrets are safely stored.
    /// The `#[serde(skip_serializing)]` attributes ensure secrets are omitted
    /// from the on-disk file automatically.
    pub fn persist_secrets(&self) {
        match &self.auth {
            AuthConfig::OAuth2 { client_secret, refresh_token, access_token, .. } => {
                if let Some(s) = client_secret {
                    if !s.is_empty() {
                        crate::secrets::save(&self.id, "client_secret", s);
                    }
                }
                if !refresh_token.is_empty() {
                    crate::secrets::save(&self.id, "refresh_token", refresh_token);
                }
                if let Some(s) = access_token {
                    if !s.is_empty() {
                        crate::secrets::save(&self.id, "access_token", s);
                    }
                }
            }
            AuthConfig::Password { password, .. } => {
                if !password.is_empty() {
                    crate::secrets::save(&self.id, "password", password);
                }
            }
            AuthConfig::BotToken { bot_token, app_token, .. } => {
                if !bot_token.is_empty() {
                    crate::secrets::save(&self.id, "bot_token", bot_token);
                }
                if let Some(s) = app_token {
                    if !s.is_empty() {
                        crate::secrets::save(&self.id, "app_token", s);
                    }
                }
            }
            AuthConfig::CdpApiKey { private_key, key_name } => {
                if !private_key.is_empty() {
                    crate::secrets::save(&self.id, "private_key", private_key);
                    tracing::debug!(
                        connector_id = %self.id,
                        key_name = %key_name,
                        "persisted private_key to keyring"
                    );
                } else {
                    tracing::debug!(
                        connector_id = %self.id,
                        "skipping empty private_key in persist_secrets"
                    );
                }
            }
            AuthConfig::Local => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::connectors::ConnectorProvider;

    #[test]
    fn microsoft_scopes_all_services() {
        let cfg = ConnectorConfig {
            id: "test".into(),
            name: "Test".into(),
            provider: ConnectorProvider::Microsoft,
            enabled: true,
            auth: AuthConfig::OAuth2 {
                client_id: "".into(),
                client_secret: None,
                refresh_token: "tok".into(),
                access_token: None,
                token_url: None,
            },
            services: ServicesConfig {
                communication: Some(CommunicationConfig {
                    enabled: true,
                    from_address: Some("me@test.com".into()),
                    folder: "inbox".into(),
                    poll_interval_secs: None,
                    default_input_class: DataClass::Internal,
                    default_output_class: DataClass::Internal,
                    destination_rules: vec![],
                    allowed_guild_ids: vec![],
                    listen_channel_ids: vec![],
                    default_send_channel_id: None,
                }),
                calendar: Some(CalendarConfig {
                    enabled: true,
                    default_class: DataClass::Internal,
                    resource_rules: vec![],
                }),
                drive: Some(DriveConfig {
                    enabled: true,
                    default_class: DataClass::Confidential,
                    resource_rules: vec![],
                }),
                contacts: Some(ContactsConfig {
                    enabled: true,
                    default_class: DataClass::Internal,
                    resource_rules: vec![],
                }),
                trading: None,
                custom: Default::default(),
            },
            allowed_personas: Vec::new(),
        };

        let scopes = cfg.required_oauth_scopes().unwrap();
        assert!(scopes.contains(&"offline_access"));
        assert!(scopes.contains(&"https://graph.microsoft.com/Mail.Send"));
        assert!(scopes.contains(&"https://graph.microsoft.com/Calendars.ReadWrite"));
        assert!(scopes.contains(&"https://graph.microsoft.com/Files.ReadWrite.All"));
        assert!(scopes.contains(&"https://graph.microsoft.com/Contacts.Read"));
    }

    #[test]
    fn microsoft_scopes_comm_only() {
        let cfg = ConnectorConfig {
            id: "test".into(),
            name: "Test".into(),
            provider: ConnectorProvider::Microsoft,
            enabled: true,
            auth: AuthConfig::OAuth2 {
                client_id: "".into(),
                client_secret: None,
                refresh_token: "tok".into(),
                access_token: None,
                token_url: None,
            },
            services: ServicesConfig {
                communication: Some(CommunicationConfig {
                    enabled: true,
                    from_address: Some("me@test.com".into()),
                    folder: "inbox".into(),
                    poll_interval_secs: None,
                    default_input_class: DataClass::Internal,
                    default_output_class: DataClass::Internal,
                    destination_rules: vec![],
                    allowed_guild_ids: vec![],
                    listen_channel_ids: vec![],
                    default_send_channel_id: None,
                }),
                calendar: None,
                drive: None,
                contacts: Some(ContactsConfig {
                    enabled: true,
                    default_class: DataClass::Internal,
                    resource_rules: vec![],
                }),
                trading: None,
                custom: Default::default(),
            },
            allowed_personas: Vec::new(),
        };

        let scopes = cfg.required_oauth_scopes().unwrap();
        assert!(scopes.contains(&"https://graph.microsoft.com/Mail.Send"));
        assert!(!scopes.contains(&"https://graph.microsoft.com/Calendars.ReadWrite"));
        assert!(!scopes.contains(&"https://graph.microsoft.com/Files.ReadWrite.All"));
    }

    #[test]
    fn discord_no_oauth_scopes() {
        let cfg = ConnectorConfig {
            id: "test".into(),
            name: "Test".into(),
            provider: ConnectorProvider::Discord,
            enabled: true,
            auth: AuthConfig::BotToken { bot_token: "tok".into(), app_token: None },
            services: ServicesConfig::default(),
            allowed_personas: Vec::new(),
        };

        assert!(cfg.required_oauth_scopes().is_none());
    }

    #[test]
    fn config_yaml_round_trip() {
        let cfg = ConnectorConfig {
            id: "work-ms".into(),
            name: "Work Microsoft".into(),
            provider: ConnectorProvider::Microsoft,
            enabled: true,
            auth: AuthConfig::OAuth2 {
                client_id: "abc".into(),
                client_secret: None,
                refresh_token: String::new(),
                access_token: None,
                token_url: None,
            },
            services: ServicesConfig {
                communication: Some(CommunicationConfig {
                    enabled: true,
                    from_address: Some("me@work.com".into()),
                    folder: "inbox".into(),
                    poll_interval_secs: Some(60),
                    default_input_class: DataClass::Internal,
                    default_output_class: DataClass::Internal,
                    destination_rules: vec![],
                    allowed_guild_ids: vec![],
                    listen_channel_ids: vec![],
                    default_send_channel_id: None,
                }),
                calendar: Some(CalendarConfig {
                    enabled: true,
                    default_class: DataClass::Internal,
                    resource_rules: vec![],
                }),
                drive: None,
                contacts: None,
                trading: None,
                custom: Default::default(),
            },
            allowed_personas: Vec::new(),
        };

        let yaml = serde_json::to_string_pretty(&cfg).unwrap();
        let back: ConnectorConfig = serde_json::from_str(&yaml).unwrap();
        assert_eq!(back.id, "work-ms");
        assert_eq!(back.provider, ConnectorProvider::Microsoft);
        assert!(back.services.communication.is_some());
        assert!(back.services.calendar.is_some());
        assert!(back.services.drive.is_none());
    }

    #[test]
    fn ui_json_round_trip() {
        // This is the exact shape the frontend sends when creating a Microsoft connector
        let json = r#"[{
            "id": "test-uuid",
            "name": "My Microsoft",
            "provider": "microsoft",
            "enabled": true,
            "auth": { "type": "oauth2" },
            "services": {
                "communication": {
                    "enabled": true,
                    "from_address": "",
                    "folder": "INBOX",
                    "poll_interval_secs": 60,
                    "default_input_class": "internal",
                    "default_output_class": "internal",
                    "destination_rules": [],
                    "allowed_guild_ids": [],
                    "listen_channel_ids": [],
                    "default_send_channel_id": null
                },
                "calendar": { "enabled": true, "default_class": "internal", "resource_rules": [] },
                "drive": { "enabled": true, "default_class": "internal", "resource_rules": [] },
                "contacts": { "enabled": true, "default_class": "internal", "resource_rules": [] }
            }
        }]"#;
        let configs: Vec<ConnectorConfig> = serde_json::from_str(json).unwrap();
        assert_eq!(configs.len(), 1);
        assert_eq!(configs[0].provider, ConnectorProvider::Microsoft);
        assert!(matches!(configs[0].auth, AuthConfig::OAuth2 { .. }));
        assert!(configs[0].services.communication.is_some());
        assert!(configs[0].services.calendar.is_some());
        assert!(configs[0].services.drive.is_some());
        assert!(configs[0].services.contacts.is_some());

        // Also verify bot-token deserializes correctly (Discord)
        let discord_json = r#"{
            "id": "d1",
            "name": "My Discord",
            "provider": "discord",
            "enabled": true,
            "auth": { "type": "bot-token", "bot_token": "tok123" },
            "services": {}
        }"#;
        let d: ConnectorConfig = serde_json::from_str(discord_json).unwrap();
        assert_eq!(d.provider, ConnectorProvider::Discord);
        assert!(matches!(d.auth, AuthConfig::BotToken { .. }));
    }

    #[test]
    fn apple_local_auth_round_trip() {
        let json = r#"{
            "id": "apple-1",
            "name": "My Apple",
            "provider": "apple",
            "enabled": true,
            "auth": { "type": "local" },
            "services": {
                "calendar": { "enabled": true, "default_class": "internal", "resource_rules": [] },
                "contacts": { "enabled": true, "default_class": "internal", "resource_rules": [] }
            }
        }"#;
        let cfg: ConnectorConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.provider, ConnectorProvider::Apple);
        assert!(matches!(cfg.auth, AuthConfig::Local));
        assert!(cfg.services.calendar.is_some());
        assert!(cfg.services.contacts.is_some());
        assert!(cfg.services.communication.is_none());
        assert!(cfg.services.drive.is_none());

        // Local auth always counts as having credentials (no secrets needed)
        assert!(cfg.has_credentials());
        let serialized = serde_json::to_string(&cfg).unwrap();
        let back: ConnectorConfig = serde_json::from_str(&serialized).unwrap();
        assert!(matches!(back.auth, AuthConfig::Local));
    }

    #[test]
    fn cdp_api_key_serialization_strips_private_key() {
        let cfg = ConnectorConfig {
            id: "coinbase".into(),
            name: "Coinbase".into(),
            provider: ConnectorProvider::Coinbase,
            enabled: true,
            auth: AuthConfig::CdpApiKey {
                key_name: "orgs/1/apiKeys/2".into(),
                private_key: "-----BEGIN PRIVATE KEY-----\nSECRET\n-----END PRIVATE KEY-----"
                    .into(),
            },
            services: ServicesConfig {
                trading: Some(TradingConfig {
                    enabled: true,
                    sandbox: false,
                    default_input_class: DataClass::Internal,
                    default_output_class: DataClass::Internal,
                }),
                ..Default::default()
            },
            allowed_personas: vec![],
        };

        // Serialized JSON must NOT contain private_key
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("SECRET"), "private_key should be skip_serializing");
        assert!(json.contains("orgs/1/apiKeys/2"), "key_name should be serialized");

        // Deserialize back — private_key should default to ""
        let back: ConnectorConfig = serde_json::from_str(&json).unwrap();
        match &back.auth {
            AuthConfig::CdpApiKey { key_name, private_key } => {
                assert_eq!(key_name, "orgs/1/apiKeys/2");
                assert!(
                    private_key.is_empty(),
                    "private_key should default to empty after round-trip"
                );
            }
            _ => panic!("expected CdpApiKey variant"),
        }

        // Also verify YAML is handled by the daemon (not tested here since serde_yaml
        // isn't a direct dependency of hive-connectors)
    }

    #[test]
    fn cdp_api_key_from_frontend_json_without_private_key() {
        // When the frontend sends a save after editing, private_key is absent
        let json = r#"{
            "id": "coinbase",
            "name": "Coinbase",
            "provider": "coinbase",
            "enabled": true,
            "auth": { "type": "cdp-api-key", "key_name": "orgs/1/apiKeys/2" },
            "services": { "trading": { "enabled": true, "sandbox": false } }
        }"#;
        let cfg: ConnectorConfig = serde_json::from_str(json).unwrap();
        match &cfg.auth {
            AuthConfig::CdpApiKey { key_name, private_key } => {
                assert_eq!(key_name, "orgs/1/apiKeys/2");
                assert!(private_key.is_empty(), "missing private_key should default to empty");
            }
            _ => panic!("expected CdpApiKey variant"),
        }
    }
}
