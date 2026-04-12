use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use parking_lot::RwLock;
use serde_json::json;
use tokio::sync::Notify;
use tracing::{debug, info, warn, Instrument};
use uuid::Uuid;

use hive_classification::DataClass;
use hive_contracts::comms::MessageDirection;
use hive_contracts::connectors::{ConnectorInfo, ConnectorMessage, ConnectorProvider, ServiceType};
use hive_contracts::ToolApproval;
use hive_core::EventBus;

use crate::audit::{
    body_hash, body_preview, now_ms, AuditStore, ConnectorAuditFilter, ConnectorAuditLog,
};
use crate::config::{AuthConfig, ConnectorConfig};
use crate::connector::Connector;
use crate::registry::ConnectorRegistry;
use crate::resolver::ResourceResolver;

use hive_contracts::connectors::ServiceAuditEntry;

// ---------------------------------------------------------------------------
// ConnectorServiceHandle — trait for output data-class enforcement
// ---------------------------------------------------------------------------

/// Opaque handle that lets downstream crates (e.g. `hive-tools`) call
/// `resolve_output_class` without depending on `ConnectorService` directly.
pub trait ConnectorServiceHandle: Send + Sync {
    fn resolve_output_class(&self, connector_id: &str, destination: &str) -> Option<DataClass>;
    /// Returns the resolved approval decision for a destination on a connector.
    fn resolve_destination_approval(
        &self,
        connector_id: &str,
        destination: &str,
    ) -> Option<ToolApproval>;
}

// ---------------------------------------------------------------------------
// ConnectorHandle — connector + resolver for a single connector
// ---------------------------------------------------------------------------

struct ConnectorHandle {
    config: ConnectorConfig,
    connector: Arc<dyn Connector>,
    resolver: ResourceResolver,
}

// ---------------------------------------------------------------------------
// ConnectorService
// ---------------------------------------------------------------------------

/// Top-level lifecycle manager for connectors.
///
/// Analogous to `CommService` in `hive-comms`: holds a map of connector
/// handles (config + connector instance + resource resolver), provides
/// methods for listing, sending, reading, auditing, and background polling.
pub struct ConnectorService {
    handles: Arc<RwLock<HashMap<String, ConnectorHandle>>>,
    registry: Arc<ConnectorRegistry>,
    audit_log: Arc<ConnectorAuditLog>,
    connectors_dir: PathBuf,
    polling: Arc<AtomicBool>,
    /// Epoch counter to deduplicate poll task generations.
    poll_epoch: Arc<std::sync::atomic::AtomicU64>,
    /// Notified when polling should stop so IDLE tasks wake up immediately.
    shutdown_notify: Arc<Notify>,
    /// Event bus for publishing inbound message events.
    event_bus: Option<Arc<EventBus>>,
}

impl ConnectorService {
    /// Create a new ConnectorService.
    ///
    /// * `connectors_dir` — directory for per-connector state and audit DB.
    pub fn new(connectors_dir: impl AsRef<Path>) -> Result<Self> {
        let connectors_dir = connectors_dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&connectors_dir)
            .with_context(|| format!("creating connectors dir {}", connectors_dir.display()))?;

        let audit_db_path = connectors_dir.join("audit.db");
        let audit_log = Arc::new(
            ConnectorAuditLog::open(&audit_db_path).context("opening connector audit log")?,
        );

        Ok(Self {
            handles: Arc::new(RwLock::new(HashMap::new())),
            registry: Arc::new(ConnectorRegistry::new()),
            audit_log,
            connectors_dir,
            polling: Arc::new(AtomicBool::new(false)),
            poll_epoch: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            shutdown_notify: Arc::new(Notify::new()),
            event_bus: None,
        })
    }

    /// Set the event bus for publishing inbound message events.
    pub fn set_event_bus(&mut self, bus: Arc<EventBus>) {
        self.event_bus = Some(bus);
    }

    /// Returns a clone of the connector registry Arc.
    pub fn registry(&self) -> Arc<ConnectorRegistry> {
        Arc::clone(&self.registry)
    }

    /// Returns a clone of the audit log Arc.
    pub fn audit_log(&self) -> Arc<ConnectorAuditLog> {
        Arc::clone(&self.audit_log)
    }

    /// Load/reload connector configurations. Replaces all existing connectors.
    pub fn load_connectors(&self, configs: Vec<ConnectorConfig>) -> Result<()> {
        let mut new_handles = HashMap::new();

        for mut cfg in configs {
            if !cfg.enabled {
                info!(connector_id = %cfg.id, "skipping disabled connector");
                continue;
            }

            // Restore secrets from the OS keyring — YAML and UI payloads
            // have secrets stripped via #[serde(skip_serializing)].
            cfg.restore_secrets();

            let connector = match Self::create_connector(&cfg, &self.connectors_dir) {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        connector_id = %cfg.id,
                        provider = %cfg.provider.as_str(),
                        error = %e,
                        "failed to create connector, skipping"
                    );
                    continue;
                }
            };

            // Build a ResourceResolver from the communication destination rules
            // (primary service), falling back to empty rules.
            let (rules, default_input, default_output) = match &cfg.services.communication {
                Some(comm) if comm.enabled => (
                    comm.destination_rules.clone(),
                    comm.default_input_class,
                    comm.default_output_class,
                ),
                _ => (Vec::new(), DataClass::Internal, DataClass::Internal),
            };
            let resolver = ResourceResolver::new(rules, default_input, default_output);

            // Register in the registry
            self.registry
                .register_with_personas(Arc::clone(&connector), cfg.allowed_personas.clone());

            info!(
                connector_id = %cfg.id,
                provider = %cfg.provider.as_str(),
                services = ?connector.enabled_services(),
                "loaded connector"
            );

            new_handles
                .insert(cfg.id.clone(), ConnectorHandle { config: cfg, connector, resolver });
        }

        let count = new_handles.len();
        *self.handles.write() = new_handles;
        info!(count, "loaded connectors");
        Ok(())
    }

    /// Load a single connector temporarily (e.g. for testing during wizard).
    /// If a connector with this ID already exists, it is replaced.
    pub fn load_single_temp(&self, cfg: &ConnectorConfig) -> Result<()> {
        let connector = Self::create_connector(cfg, &self.connectors_dir)?;
        let (rules, default_input, default_output) = match &cfg.services.communication {
            Some(comm) if comm.enabled => (
                comm.destination_rules.clone(),
                comm.default_input_class,
                comm.default_output_class,
            ),
            _ => (Vec::new(), DataClass::Internal, DataClass::Internal),
        };
        let resolver = ResourceResolver::new(rules, default_input, default_output);
        self.registry.register_with_personas(Arc::clone(&connector), cfg.allowed_personas.clone());
        self.handles
            .write()
            .insert(cfg.id.clone(), ConnectorHandle { config: cfg.clone(), connector, resolver });
        Ok(())
    }

    /// Create a connector instance from configuration.
    fn create_connector(
        config: &ConnectorConfig,
        connectors_dir: &Path,
    ) -> Result<Arc<dyn Connector>> {
        match config.provider {
            ConnectorProvider::Microsoft => Self::create_microsoft_connector(config),
            ConnectorProvider::Discord => Self::create_discord_connector(config),
            ConnectorProvider::Slack => Self::create_slack_connector(config),
            ConnectorProvider::Gmail => Self::create_gmail_connector(config, connectors_dir),
            ConnectorProvider::Imap => Self::create_imap_connector(config, connectors_dir),
            ConnectorProvider::Coinbase => Self::create_coinbase_connector(config),
            ConnectorProvider::Apple => Self::create_apple_connector(config),
            provider => {
                bail!(
                    "connector provider '{}' is not yet implemented in hive-connectors",
                    provider.as_str()
                );
            }
        }
    }

    /// Build a MicrosoftConnector from config, wiring up enabled sub-services.
    fn create_microsoft_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        use crate::providers::microsoft::calendar::MicrosoftCalendar;
        use crate::providers::microsoft::communication::MicrosoftCommunication;
        use crate::providers::microsoft::contacts::MicrosoftContacts;
        use crate::providers::microsoft::drive::MicrosoftDrive;
        use crate::providers::microsoft::graph_client::GraphClient;
        use crate::providers::microsoft::MicrosoftConnector;

        let (client_id, refresh_token, access_token) = match &config.auth {
            AuthConfig::OAuth2 { client_id, refresh_token, access_token, .. } => {
                (client_id.clone(), refresh_token.clone(), access_token.clone())
            }
            _ => bail!("Microsoft connector '{}' requires OAuth2 auth config", config.id),
        };

        let scopes = config.required_oauth_scopes().unwrap_or_default().join(" ");

        let graph = Arc::new(GraphClient::new(
            &config.id,
            &client_id,
            &refresh_token,
            access_token.as_deref(),
            &scopes,
        ));

        let mut enabled_services = Vec::new();

        // Communication
        let communication = config.services.communication.as_ref().filter(|c| c.enabled).map(|c| {
            enabled_services.push(ServiceType::Communication);
            MicrosoftCommunication::new(
                Arc::clone(&graph),
                c.from_address.as_deref().unwrap_or(""),
                &c.folder,
            )
        });

        // Calendar
        let calendar = config.services.calendar.as_ref().filter(|c| c.enabled).map(|c| {
            enabled_services.push(ServiceType::Calendar);
            MicrosoftCalendar::new(Arc::clone(&graph), c.default_class)
        });

        // Drive
        let drive = config.services.drive.as_ref().filter(|d| d.enabled).map(|d| {
            enabled_services.push(ServiceType::Drive);
            MicrosoftDrive::new(Arc::clone(&graph), d.default_class)
        });

        // Contacts
        let contacts = config.services.contacts.as_ref().filter(|c| c.enabled).map(|c| {
            enabled_services.push(ServiceType::Contacts);
            MicrosoftContacts::new(Arc::clone(&graph), c.default_class)
        });

        Ok(Arc::new(MicrosoftConnector::new(
            &config.id,
            &config.name,
            graph,
            communication,
            calendar,
            drive,
            contacts,
            enabled_services,
        )))
    }

    /// Build a DiscordConnector from config.
    fn create_discord_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        use crate::providers::discord::communication::DiscordCommunication;
        use crate::providers::discord::DiscordConnector;

        let bot_token = match &config.auth {
            AuthConfig::BotToken { bot_token, .. } => bot_token.clone(),
            _ => bail!("Discord connector '{}' requires BotToken auth config", config.id),
        };

        let communication = config.services.communication.as_ref().filter(|c| c.enabled).map(|c| {
            DiscordCommunication::new(
                &config.id,
                bot_token.clone(),
                c.allowed_guild_ids.clone(),
                c.listen_channel_ids.clone(),
                c.default_send_channel_id.clone(),
            )
        });

        Ok(Arc::new(DiscordConnector::new(&config.id, &config.name, communication)))
    }

    /// Build a SlackConnector from config.
    fn create_slack_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        use crate::providers::slack::SlackConnector;

        let (bot_token, app_token) = match &config.auth {
            AuthConfig::BotToken { bot_token, app_token, .. } => {
                (bot_token.clone(), app_token.clone().unwrap_or_default())
            }
            _ => bail!("Slack connector '{}' requires BotToken auth config", config.id),
        };

        let comm_cfg = config.services.communication.as_ref().filter(|c| c.enabled);
        let listen_channel_ids = comm_cfg.map(|c| c.listen_channel_ids.clone()).unwrap_or_default();
        let default_send_channel_id = comm_cfg.and_then(|c| c.default_send_channel_id.clone());

        Ok(Arc::new(SlackConnector::new_deferred(
            &config.id,
            &config.name,
            bot_token,
            app_token,
            listen_channel_ids,
            default_send_channel_id,
        )))
    }

    /// Build a GmailConnector from config.
    fn create_gmail_connector(
        config: &ConnectorConfig,
        _connectors_dir: &Path,
    ) -> Result<Arc<dyn Connector>> {
        use crate::providers::gmail::calendar::GoogleCalendar;
        use crate::providers::gmail::communication::GmailCommunication;
        use crate::providers::gmail::contacts::GoogleContacts;
        use crate::providers::gmail::drive::GoogleDrive;
        use crate::providers::gmail::google_client::GoogleClient;
        use crate::providers::gmail::GmailConnector;

        let (client_id, client_secret, refresh_token, access_token) = match &config.auth {
            AuthConfig::OAuth2 {
                client_id, client_secret, refresh_token, access_token, ..
            } => (
                client_id.clone(),
                client_secret.clone().unwrap_or_default(),
                refresh_token.clone(),
                access_token.clone(),
            ),
            _ => bail!("Gmail connector '{}' requires OAuth2 auth config", config.id),
        };

        let scopes = config.required_oauth_scopes().unwrap_or_default().join(" ");

        let google = Arc::new(GoogleClient::new(
            &config.id,
            &client_id,
            &client_secret,
            &refresh_token,
            access_token.as_deref(),
            &scopes,
        ));

        let mut enabled_services = Vec::new();

        let communication =
            config.services.communication.as_ref().filter(|c| c.enabled).and_then(|c| {
                let from_address = c.from_address.as_deref().unwrap_or_default();
                if from_address.is_empty() {
                    tracing::warn!(
                        connector = %config.id,
                        "Gmail communication enabled but no from_address configured"
                    );
                    None
                } else {
                    enabled_services.push(ServiceType::Communication);
                    Some(GmailCommunication::new(
                        Arc::clone(&google),
                        &config.id,
                        from_address,
                        &c.folder,
                    ))
                }
            });

        let calendar = config.services.calendar.as_ref().filter(|c| c.enabled).map(|c| {
            enabled_services.push(ServiceType::Calendar);
            GoogleCalendar::new(Arc::clone(&google), c.default_class)
        });

        let drive = config.services.drive.as_ref().filter(|d| d.enabled).map(|d| {
            enabled_services.push(ServiceType::Drive);
            GoogleDrive::new(Arc::clone(&google), d.default_class)
        });

        let contacts = config.services.contacts.as_ref().filter(|c| c.enabled).map(|c| {
            enabled_services.push(ServiceType::Contacts);
            GoogleContacts::new(Arc::clone(&google), c.default_class)
        });

        Ok(Arc::new(GmailConnector::new(
            &config.id,
            &config.name,
            google,
            communication,
            calendar,
            drive,
            contacts,
            enabled_services,
        )))
    }

    /// Build an ImapConnector from config.
    fn create_imap_connector(
        config: &ConnectorConfig,
        connectors_dir: &Path,
    ) -> Result<Arc<dyn Connector>> {
        use crate::providers::imap::ImapConnector;

        let connector = ImapConnector::from_config(
            &config.id,
            &config.name,
            &config.auth,
            config.services.communication.as_ref(),
            connectors_dir,
        )?;
        Ok(Arc::new(connector))
    }

    /// Build a CoinbaseConnector from config.
    fn create_coinbase_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        use crate::providers::coinbase::api::CoinbaseClient;
        use crate::providers::coinbase::trading::CoinbaseTradingService;
        use crate::providers::coinbase::CoinbaseConnector;

        let sandbox = config.services.trading.as_ref().is_some_and(|t| t.sandbox);

        let build_client = |cfg: &ConnectorConfig| -> Result<CoinbaseClient> {
            match &cfg.auth {
                AuthConfig::CdpApiKey { key_name, private_key } => {
                    CoinbaseClient::new(&cfg.id, key_name, private_key, sandbox)
                }
                _ => bail!("Coinbase connector '{}' requires CdpApiKey auth config", cfg.id),
            }
        };

        let trading = if let Some(_) = config.services.trading.as_ref().filter(|t| t.enabled) {
            let client = Arc::new(build_client(config)?);
            Some(CoinbaseTradingService::new(client))
        } else {
            None
        };

        Ok(Arc::new(CoinbaseConnector::new(&config.id, &config.name, trading)))
    }

    /// Build an AppleConnector from config (macOS only).
    fn create_apple_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        #[cfg(not(target_os = "macos"))]
        {
            let _ = config;
            bail!("Apple connector is only available on macOS");
        }

        #[cfg(target_os = "macos")]
        {
            use crate::providers::apple::calendar::AppleCalendar;
            use crate::providers::apple::connector::AppleConnector;
            use crate::providers::apple::contacts::AppleContacts;

            let mut enabled = Vec::new();

            let calendar = config.services.calendar.as_ref().filter(|c| c.enabled).map(|_| {
                enabled.push(ServiceType::Calendar);
                AppleCalendar::new(&config.id)
            });

            let contacts = config.services.contacts.as_ref().filter(|c| c.enabled).map(|_| {
                enabled.push(ServiceType::Contacts);
                AppleContacts::new(&config.id)
            });

            Ok(Arc::new(AppleConnector::new(&config.id, &config.name, calendar, contacts, enabled)))
        }
    }

    /// List available connectors, filtered to those accessible by the given persona.
    ///
    /// Pass `None` to return all connectors (no persona filtering).
    pub fn list_connectors(&self, persona_id: Option<&str>) -> Vec<ConnectorInfo> {
        let handles = self.handles.read();
        handles
            .values()
            .filter(|h| match persona_id {
                Some(pid) => h.config.allowed_personas.iter().any(|p| p == "*" || p == pid),
                None => true,
            })
            .map(|h| ConnectorInfo {
                id: h.config.id.clone(),
                name: h.config.name.clone(),
                provider: h.config.provider,
                enabled: h.config.enabled,
                status: h.connector.status(),
                enabled_services: h.connector.enabled_services(),
                allowed_personas: h.config.allowed_personas.clone(),
            })
            .collect()
    }

    /// List connectors that have communication enabled (for service registration).
    pub fn communication_connector_ids(&self) -> Vec<(String, String)> {
        let handles = self.handles.read();
        handles
            .values()
            .filter(|h| h.connector.communication().is_some())
            .map(|h| (h.config.id.clone(), h.config.name.clone()))
            .collect()
    }

    /// Return cloned configs for all loaded connectors.
    ///
    /// Secrets are omitted automatically by the `#[serde(skip_serializing)]`
    /// attributes on [`AuthConfig`] fields, so the returned configs are safe
    /// to expose over the API.
    pub fn list_connector_configs(&self) -> Vec<ConnectorConfig> {
        let handles = self.handles.read();
        handles.values().map(|h| h.config.clone()).collect()
    }

    /// Test connectivity for a loaded connector.
    ///
    /// Tests the first available service (communication, calendar, drive, contacts).
    pub async fn test_connector(&self, connector_id: &str) -> Result<()> {
        let connector = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).ok_or_else(|| {
                anyhow::anyhow!("connector '{connector_id}' not found or not loaded")
            })?;
            Arc::clone(&handle.connector)
        };

        if let Some(comm) = connector.communication() {
            return comm.test_connection().await;
        }
        if let Some(cal) = connector.calendar() {
            return cal.test_connection().await;
        }
        if let Some(drive) = connector.drive() {
            return drive.test_connection().await;
        }
        if let Some(contacts) = connector.contacts() {
            return contacts.test_connection().await;
        }

        // Fall back to the first DynService in the service registry (e.g. trading).
        if let Some(reg) = connector.service_registry() {
            let descriptors = reg.list();
            if let Some(desc) = descriptors.first() {
                if let Some(svc) = reg.get(&desc.service_type) {
                    return svc.test_connection().await;
                }
            }
        }

        bail!("connector '{connector_id}' has no enabled services to test");
    }

    /// Get the default send channel ID for a connector (if configured).
    pub fn default_send_channel_id(&self, connector_id: &str) -> Option<String> {
        let handles = self.handles.read();
        handles
            .get(connector_id)
            .and_then(|h| h.config.services.communication.as_ref())
            .and_then(|c| c.default_send_channel_id.clone())
    }

    /// Resolve the maximum allowed output data-class for a connector and
    /// destination address.  Returns `None` if the connector is not found.
    pub fn resolve_output_class(&self, connector_id: &str, destination: &str) -> Option<DataClass> {
        let handles = self.handles.read();
        handles.get(connector_id).map(|h| h.resolver.resolve(destination).output_class)
    }

    /// Send a message through a connector's communication service.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_message(
        &self,
        connector_id: &str,
        to: &[String],
        subject: Option<&str>,
        body: &str,
        attachments: &[crate::services::communication::CommAttachment],
        agent_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<ConnectorMessage> {
        // Enforce connector destination rules BEFORE sending.
        if let Some(first_to) = to.first() {
            let handles = self.handles.read();
            if let Some(handle) = handles.get(connector_id) {
                let policy = handle.resolver.resolve(first_to);
                tracing::info!(
                    connector_id,
                    destination = %first_to,
                    approval = ?policy.approval,
                    output_class = ?policy.output_class,
                    "connector destination rule resolved"
                );
                if policy.approval == ToolApproval::Deny {
                    anyhow::bail!(
                        "destination '{}' is denied by connector rule on '{}'",
                        first_to,
                        connector_id
                    );
                }
            }
        }

        let (connector, config) = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).context("connector not found")?;
            (Arc::clone(&handle.connector), handle.config.clone())
        };

        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;

        // Perform send outside the lock
        let external_id = comm.send(to, subject, body, attachments).await?;

        let msg_id = Uuid::new_v4().to_string();
        let ts = now_ms();

        // Determine output classification
        let output_class = if let Some(first_to) = to.first() {
            let handles = self.handles.read();
            handles
                .get(connector_id)
                .map(|h| h.resolver.resolve(first_to).output_class)
                .unwrap_or(DataClass::Internal)
        } else {
            DataClass::Internal
        };

        let message = ConnectorMessage {
            id: msg_id.clone(),
            connector_id: connector_id.to_string(),
            provider: config.provider,
            direction: MessageDirection::Outbound,
            from: config.email_from_address().unwrap_or_default().to_string(),
            to: to.to_vec(),
            subject: subject.map(|s| s.to_string()),
            body: body.to_string(),
            timestamp_ms: ts,
            data_class: output_class,
            metadata: {
                let mut m = HashMap::new();
                m.insert("external_id".into(), external_id);
                m
            },
        };

        // Audit log
        for to_addr in to {
            let audit_entry = ServiceAuditEntry {
                id: format!("{msg_id}-{to_addr}"),
                connector_id: connector_id.to_string(),
                provider: config.provider,
                service_type: ServiceType::Communication,
                operation: "send".into(),
                direction: Some(MessageDirection::Outbound),
                from_address: Some(message.from.clone()),
                to_address: Some(to_addr.clone()),
                subject: subject.map(|s| s.to_string()),
                resource_id: None,
                resource_name: None,
                body_hash: body_hash(body),
                body_preview: Some(body_preview(body, 200)),
                data_class: output_class,
                approval_decision: Some("approved".into()),
                agent_id: agent_id.map(|s| s.to_string()),
                session_id: session_id.map(|s| s.to_string()),
                timestamp_ms: ts,
            };
            if let Err(e) = self.audit_log.record(&audit_entry) {
                warn!(error = %e, "failed to write connector audit entry");
            }
        }

        Ok(message)
    }

    /// Send a rich/interactive message via a connector's communication service.
    ///
    /// Falls back to plain-text `send` when the provider doesn't override `send_rich`.
    #[allow(clippy::too_many_arguments)]
    pub async fn send_rich_message(
        &self,
        connector_id: &str,
        to: &[String],
        subject: Option<&str>,
        fallback_text: &str,
        rich_body: Option<crate::services::communication::RichMessageBody>,
        attachments: &[crate::services::communication::CommAttachment],
        agent_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<ConnectorMessage> {
        // Enforce connector destination rules BEFORE sending.
        if let Some(first_to) = to.first() {
            let handles = self.handles.read();
            if let Some(handle) = handles.get(connector_id) {
                let policy = handle.resolver.resolve(first_to);
                tracing::info!(
                    connector_id,
                    destination = %first_to,
                    approval = ?policy.approval,
                    output_class = ?policy.output_class,
                    "connector destination rule resolved (rich)"
                );
                if policy.approval == ToolApproval::Deny {
                    anyhow::bail!(
                        "destination '{}' is denied by connector rule on '{}'",
                        first_to,
                        connector_id
                    );
                }
            }
        }

        let (connector, config) = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).context("connector not found")?;
            (Arc::clone(&handle.connector), handle.config.clone())
        };

        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;

        let external_id =
            comm.send_rich(to, subject, fallback_text, rich_body, attachments).await?;

        let msg_id = Uuid::new_v4().to_string();
        let ts = now_ms();

        let output_class = if let Some(first_to) = to.first() {
            let handles = self.handles.read();
            handles
                .get(connector_id)
                .map(|h| h.resolver.resolve(first_to).output_class)
                .unwrap_or(DataClass::Internal)
        } else {
            DataClass::Internal
        };

        let message = ConnectorMessage {
            id: msg_id.clone(),
            connector_id: connector_id.to_string(),
            provider: config.provider,
            direction: MessageDirection::Outbound,
            from: config.email_from_address().unwrap_or_default().to_string(),
            to: to.to_vec(),
            subject: subject.map(|s| s.to_string()),
            body: fallback_text.to_string(),
            timestamp_ms: ts,
            data_class: output_class,
            metadata: {
                let mut m = HashMap::new();
                m.insert("external_id".into(), external_id);
                m
            },
        };

        for to_addr in to {
            let audit_entry = ServiceAuditEntry {
                id: format!("{msg_id}-{to_addr}"),
                connector_id: connector_id.to_string(),
                provider: config.provider,
                service_type: ServiceType::Communication,
                operation: "send_rich".into(),
                direction: Some(MessageDirection::Outbound),
                from_address: Some(message.from.clone()),
                to_address: Some(to_addr.clone()),
                subject: subject.map(|s| s.to_string()),
                resource_id: None,
                resource_name: None,
                body_hash: body_hash(fallback_text),
                body_preview: Some(body_preview(fallback_text, 200)),
                data_class: output_class,
                approval_decision: Some("approved".into()),
                agent_id: agent_id.map(|s| s.to_string()),
                session_id: session_id.map(|s| s.to_string()),
                timestamp_ms: ts,
            };
            if let Err(e) = self.audit_log.record(&audit_entry) {
                warn!(error = %e, "failed to write connector audit entry");
            }
        }

        Ok(message)
    }

    /// Read new messages from a connector's communication service.
    pub async fn read_messages(
        &self,
        connector_id: &str,
        limit: usize,
        agent_id: Option<&str>,
        session_id: Option<&str>,
    ) -> Result<Vec<ConnectorMessage>> {
        let (connector, config) = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).context("connector not found")?;
            (Arc::clone(&handle.connector), handle.config.clone())
        };

        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;

        // Fetch outside the lock
        let inbound = comm.fetch_new(limit).await?;

        let mut messages = Vec::new();

        for msg in inbound {
            let input_class = {
                let handles = self.handles.read();
                handles
                    .get(connector_id)
                    .map(|h| h.resolver.resolve(&msg.from).input_class)
                    .unwrap_or(DataClass::Internal)
            };

            let msg_id = Uuid::new_v4().to_string();
            let ts = msg.timestamp_ms;

            // Audit log
            let audit_entry = ServiceAuditEntry {
                id: msg_id.clone(),
                connector_id: connector_id.to_string(),
                provider: config.provider,
                service_type: ServiceType::Communication,
                operation: "fetch".into(),
                direction: Some(MessageDirection::Inbound),
                from_address: Some(msg.from.clone()),
                to_address: msg.to.first().cloned(),
                subject: msg.subject.clone(),
                resource_id: Some(msg.external_id.clone()),
                resource_name: None,
                body_hash: body_hash(&msg.body),
                body_preview: Some(body_preview(&msg.body, 200)),
                data_class: input_class,
                approval_decision: Some("auto".into()),
                agent_id: agent_id.map(|s| s.to_string()),
                session_id: session_id.map(|s| s.to_string()),
                timestamp_ms: ts,
            };
            if let Err(e) = self.audit_log.record(&audit_entry) {
                warn!(error = %e, "failed to write connector audit entry");
            }

            messages.push(ConnectorMessage {
                id: msg_id,
                connector_id: connector_id.to_string(),
                provider: config.provider,
                direction: MessageDirection::Inbound,
                from: msg.from,
                to: msg.to,
                subject: msg.subject,
                body: msg.body,
                timestamp_ms: ts,
                data_class: input_class,
                metadata: msg.metadata,
            });
        }

        Ok(messages)
    }

    /// Search the connector audit log.
    pub fn search_audit(&self, filter: &ConnectorAuditFilter) -> Result<Vec<ServiceAuditEntry>> {
        self.audit_log.query(filter)
    }

    /// Get the connectors directory path.
    pub fn connectors_dir(&self) -> &Path {
        &self.connectors_dir
    }

    // ── Background Polling ─────────────────────────────────────────────

    /// Mark an inbound message as seen/read on the connector provider.
    ///
    /// `connector_id` is the connector that received the message.
    /// `external_id` is the message's external identifier.
    pub async fn mark_message_seen(&self, connector_id: &str, external_id: &str) -> Result<()> {
        let connector = {
            let handles = self.handles.read();
            handles
                .get(connector_id)
                .map(|h| Arc::clone(&h.connector))
                .ok_or_else(|| anyhow::anyhow!("connector not found: {connector_id}"))?
        };
        let comm = connector
            .communication()
            .ok_or_else(|| anyhow::anyhow!("connector has no communication service"))?;
        comm.mark_seen(external_id).await
    }

    /// List available channels for a connector's communication service.
    pub async fn list_channels(
        &self,
        connector_id: &str,
    ) -> Result<Vec<crate::services::communication::ChannelInfo>> {
        let connector = {
            let handles = self.handles.read();
            handles
                .get(connector_id)
                .map(|h| Arc::clone(&h.connector))
                .ok_or_else(|| anyhow::anyhow!("connector not found: {connector_id}"))?
        };
        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;
        comm.list_channels().await
    }

    /// Start background polling for all connectors that have a communication
    /// service enabled.
    ///
    /// Each eligible connector gets its own tokio task. Connectors that support
    /// IMAP IDLE will use server-push notification; others fall back to timed
    /// polling.
    ///
    /// Call this after `load_connectors()` and after each reload.
    pub fn start_background_poll(self: &Arc<Self>) {
        // Stop any existing poll loop
        self.polling.store(false, Ordering::SeqCst);
        // Bump epoch so stale tasks from prior generations exit
        let epoch = self.poll_epoch.fetch_add(1, Ordering::SeqCst) + 1;
        let svc = Arc::clone(self);
        tokio::spawn(
            async move {
                tracing::info!("connector polling service started");
                // Small grace period so existing tasks see the flag
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                // Check that no newer generation has started
                if svc.poll_epoch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                svc.polling.store(true, Ordering::SeqCst);
                svc.spawn_poll_tasks(epoch);
            }
            .instrument(tracing::info_span!("service", service = "connector-polling")),
        );
    }

    /// Stop the background polling loop.
    pub fn stop_polling(&self) {
        self.polling.store(false, Ordering::SeqCst);
        self.shutdown_notify.notify_waiters();
    }

    /// Returns `true` if the background polling loop is active.
    pub fn is_polling(&self) -> bool {
        self.polling.load(Ordering::SeqCst)
    }

    fn spawn_poll_tasks(self: &Arc<Self>, epoch: u64) {
        let handles = self.handles.read();
        for (connector_id, handle) in handles.iter() {
            let interval_secs = handle.config.poll_interval_secs().unwrap_or(60);
            if interval_secs == 0 {
                continue;
            }

            if !handle.config.has_credentials() {
                info!(
                    connector_id = %connector_id,
                    "skipping poll for connector without credentials"
                );
                continue;
            }

            // Only poll connectors that have communication enabled
            let comm = match handle.connector.communication() {
                Some(c) => c,
                None => continue,
            };

            let uses_idle = comm.supports_idle();
            let svc = Arc::clone(self);
            let cid = connector_id.clone();
            let poll_interval = std::time::Duration::from_secs(interval_secs);
            let poll_epoch = Arc::clone(&self.poll_epoch);
            let shutdown = Arc::clone(&self.shutdown_notify);

            info!(
                connector_id = %cid,
                interval_secs,
                uses_idle,
                "starting background listener for connector"
            );

            // Each per-connector task gets its own `service = <connector_id>` span
            // so that ServiceLogCollector routes its logs to the individual
            // ConnectorListenerDaemonService instead of the umbrella polling service.
            let span_service_id = cid.clone();
            tokio::spawn(async move {
                let mut consecutive_failures: u32 = 0;
                const MAX_BACKOFF_SECS: u64 = 300; // 5 min cap

                loop {
                    if !svc.polling.load(Ordering::SeqCst)
                        || poll_epoch.load(Ordering::SeqCst) != epoch
                    {
                        debug!(connector_id = %cid, "poll loop stopped (flag or epoch mismatch)");
                        break;
                    }

                    if svc.poll_connector_once(&cid).await {
                        consecutive_failures = 0;
                    } else {
                        consecutive_failures = consecutive_failures.saturating_add(1);
                    }

                    // Back off on consecutive failures
                    if consecutive_failures > 0 {
                        let backoff = poll_interval
                            .max(std::time::Duration::from_secs(30))
                            .min(std::time::Duration::from_secs(MAX_BACKOFF_SECS))
                            * consecutive_failures.min(6);
                        tokio::select! {
                            _ = tokio::time::sleep(backoff) => {}
                            _ = shutdown.notified() => {
                                debug!(connector_id = %cid, "shutdown during backoff");
                                break;
                            }
                        }
                        continue;
                    }

                    // Wait for next cycle: IDLE (server push) or timed sleep
                    if uses_idle {
                        let idle_timeout = poll_interval.min(std::time::Duration::from_secs(29 * 60));
                        let connector = {
                            let handles = svc.handles.read();
                            handles.get(&cid).map(|h| Arc::clone(&h.connector))
                        };
                        if let Some(connector) = connector {
                            if let Some(comm) = connector.communication() {
                                tokio::select! {
                                    result = comm.wait_for_changes(idle_timeout) => {
                                        match result {
                                            Ok(has_new) => {
                                                if has_new {
                                                    debug!(connector_id = %cid, "IDLE: server signalled new mail");
                                                }
                                            }
                                            Err(e) => {
                                                warn!(connector_id = %cid, error = %e, "IDLE failed, falling back to timed poll");
                                                // Short sleep before retry; also cancellable
                                                tokio::select! {
                                                    _ = tokio::time::sleep(poll_interval) => {}
                                                    _ = shutdown.notified() => { break; }
                                                }
                                            }
                                        }
                                    }
                                    _ = shutdown.notified() => {
                                        debug!(connector_id = %cid, "shutdown during IDLE");
                                        break;
                                    }
                                }
                            } else {
                                tokio::select! {
                                    _ = tokio::time::sleep(poll_interval) => {}
                                    _ = shutdown.notified() => { break; }
                                }
                            }
                        } else {
                            tokio::select! {
                                _ = tokio::time::sleep(poll_interval) => {}
                                _ = shutdown.notified() => { break; }
                            }
                        }
                    } else {
                        tokio::select! {
                            _ = tokio::time::sleep(poll_interval) => {}
                            _ = shutdown.notified() => {
                                debug!(connector_id = %cid, "shutdown during sleep");
                                break;
                            }
                        }
                    }
                }
            }.instrument(tracing::info_span!("service", service = %span_service_id)));
        }
    }

    /// Directly invoke `create_connector` for testability.
    #[cfg(test)]
    fn try_create_connector(config: &ConnectorConfig) -> Result<Arc<dyn Connector>> {
        Self::create_connector(config, Path::new("/tmp"))
    }

    async fn poll_connector_once(&self, connector_id: &str) -> bool {
        let (connector, config) = {
            let handles = self.handles.read();
            match handles.get(connector_id) {
                Some(h) => (Arc::clone(&h.connector), h.config.clone()),
                None => return false,
            }
        };

        let comm = match connector.communication() {
            Some(c) => c,
            None => return false,
        };

        let inbound = match comm.fetch_new(50).await {
            Ok(msgs) => msgs,
            Err(e) => {
                warn!(connector_id, error = %e, "background poll failed");
                return false;
            }
        };

        if inbound.is_empty() {
            return true;
        }

        info!(connector_id, count = inbound.len(), "background poll found new messages");

        for msg in &inbound {
            let input_class = {
                let handles = self.handles.read();
                handles
                    .get(connector_id)
                    .map(|h| h.resolver.resolve(&msg.from).input_class)
                    .unwrap_or(DataClass::Internal)
            };

            let msg_id = Uuid::new_v4().to_string();
            let ts = msg.timestamp_ms;

            let audit_entry = ServiceAuditEntry {
                id: msg_id,
                connector_id: connector_id.to_string(),
                provider: config.provider,
                service_type: ServiceType::Communication,
                operation: "poll".into(),
                direction: Some(MessageDirection::Inbound),
                from_address: Some(msg.from.clone()),
                to_address: msg.to.first().cloned(),
                subject: msg.subject.clone(),
                resource_id: Some(msg.external_id.clone()),
                resource_name: None,
                body_hash: body_hash(&msg.body),
                body_preview: Some(body_preview(&msg.body, 200)),
                data_class: input_class,
                approval_decision: Some("auto".into()),
                agent_id: None,
                session_id: None,
                timestamp_ms: ts,
            };
            if let Err(e) = self.audit_log.record(&audit_entry) {
                warn!(error = %e, "failed to write connector audit entry during poll");
            }

            // Publish inbound message event for workflow triggers
            if let Some(ref bus) = self.event_bus {
                let topic = format!("comm.message.received.{connector_id}");
                let payload = json!({
                    "channel_id": connector_id,
                    "provider": format!("{:?}", config.provider).to_lowercase(),
                    "external_id": msg.external_id,
                    "from": msg.from,
                    "to": msg.to,
                    "subject": msg.subject,
                    "body": msg.body,
                    "timestamp_ms": msg.timestamp_ms as u64,
                    "metadata": msg.metadata,
                });
                if let Err(e) = bus.publish(&topic, "connector-poll", payload) {
                    warn!(error = %e, connector_id, "failed to publish inbound message event");
                }
            }
        }
        true
    }

    /// Edit a previously-sent message on a connector's channel.
    /// `channel_id` and `message_id` are provider-specific (e.g. Slack channel + ts).
    pub async fn edit_message(
        &self,
        connector_id: &str,
        channel_id: &str,
        message_id: &str,
        new_text: &str,
    ) -> Result<()> {
        let connector = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).context("connector not found")?;
            Arc::clone(&handle.connector)
        };

        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;

        comm.edit_message(channel_id, message_id, new_text).await
    }

    /// Acknowledge a Discord-style interaction (button click callback).
    pub async fn acknowledge_interaction(
        &self,
        connector_id: &str,
        interaction_id: &str,
        interaction_token: &str,
        update_content: &str,
    ) -> Result<()> {
        let connector = {
            let handles = self.handles.read();
            let handle = handles.get(connector_id).context("connector not found")?;
            Arc::clone(&handle.connector)
        };

        let comm = connector.communication().ok_or_else(|| {
            anyhow::anyhow!("connector '{connector_id}' has no communication service")
        })?;

        comm.acknowledge_interaction(interaction_id, interaction_token, update_content).await
    }
}

impl ConnectorServiceHandle for ConnectorService {
    fn resolve_output_class(&self, connector_id: &str, destination: &str) -> Option<DataClass> {
        self.resolve_output_class(connector_id, destination)
    }

    fn resolve_destination_approval(
        &self,
        connector_id: &str,
        destination: &str,
    ) -> Option<ToolApproval> {
        let handles = self.handles.read();
        handles.get(connector_id).map(|h| h.resolver.resolve(destination).approval)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AuthConfig, CalendarConfig, ConnectorConfig, ContactsConfig, ServicesConfig,
    };
    use hive_contracts::connectors::ConnectorProvider;
    use tempfile::TempDir;

    /// Build a minimal Microsoft connector config (has OAuth2 auth so
    /// `create_connector` can succeed for the Microsoft path).
    fn microsoft_config(id: &str, name: &str) -> ConnectorConfig {
        ConnectorConfig {
            id: id.into(),
            name: name.into(),
            provider: ConnectorProvider::Microsoft,
            enabled: true,
            auth: AuthConfig::OAuth2 {
                client_id: "test-client".into(),
                client_secret: None,
                refresh_token: "test-refresh".into(),
                access_token: None,
                token_url: None,
            },
            services: ServicesConfig::default(),
            allowed_personas: Vec::new(),
        }
    }

    /// Build a config whose provider is not yet implemented (Telegram is
    /// listed in the enum but has no factory method).
    fn unsupported_config(id: &str) -> ConnectorConfig {
        ConnectorConfig {
            id: id.into(),
            name: "Unsupported".into(),
            provider: ConnectorProvider::Telegram,
            enabled: true,
            auth: AuthConfig::OAuth2 {
                client_id: "x".into(),
                client_secret: None,
                refresh_token: "x".into(),
                access_token: None,
                token_url: None,
            },
            services: ServicesConfig::default(),
            allowed_personas: Vec::new(),
        }
    }

    // ── 1. new() creates directory and audit.db ─────────────────────────

    #[test]
    fn test_new_creates_dir() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("connectors");
        assert!(!dir.exists());

        let _svc = ConnectorService::new(&dir).unwrap();

        assert!(dir.exists());
        assert!(dir.join("audit.db").exists());
    }

    // ── 2. registry starts empty ────────────────────────────────────────

    #[test]
    fn test_registry_starts_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        assert!(svc.registry().list().is_empty());
    }

    // ── 3. list_connectors empty before loading ─────────────────────────

    #[test]
    fn test_list_connectors_empty() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        assert!(svc.list_connectors(None).is_empty());
    }

    // ── 4. list_connectors filtered by persona ────────────────────────

    #[test]
    fn test_list_connectors_filtered() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        let mut cfg_work = microsoft_config("ms-work", "Work");
        cfg_work.allowed_personas = vec!["system/general".into()];
        let cfg_personal = microsoft_config("ms-personal", "Personal");
        // ms-personal has no allowed personas

        svc.load_connectors(vec![cfg_work, cfg_personal]).unwrap();

        // Unfiltered — returns all
        let all = svc.list_connectors(None);
        assert_eq!(all.len(), 2);

        // Filter by persona that only ms-work allows
        let filtered = svc.list_connectors(Some("system/general"));
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "ms-work");

        // Filter by persona that no connector allows
        let none = svc.list_connectors(Some("user/nonexistent"));
        assert!(none.is_empty());
    }

    // ── 5. unsupported provider ─────────────────────────────────────────

    #[test]
    fn test_unsupported_provider() {
        // Directly calling create_connector for an unsupported provider errors
        let cfg = unsupported_config("gmail-test");
        let result = ConnectorService::try_create_connector(&cfg);
        let err = result.err().expect("expected an error for unsupported provider");
        let err_msg = err.to_string();
        assert!(err_msg.contains("not yet implemented"), "unexpected error: {err_msg}");
    }

    #[test]
    fn test_load_connectors_skips_unsupported_provider() {
        // load_connectors logs a warning but does not fail
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        let cfgs = vec![microsoft_config("ms-ok", "OK"), unsupported_config("gmail-bad")];
        // Should not error — the unsupported config is skipped
        svc.load_connectors(cfgs).unwrap();

        // Only the Microsoft connector is loaded
        let list = svc.list_connectors(None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "ms-ok");
    }

    #[test]
    fn test_load_connectors_skips_disabled() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        let mut cfg = microsoft_config("ms-disabled", "Disabled");
        cfg.enabled = false;

        svc.load_connectors(vec![cfg]).unwrap();
        assert!(svc.list_connectors(None).is_empty());
        assert!(svc.registry().list().is_empty());
    }

    #[test]
    fn test_load_replaces_previous() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        svc.load_connectors(vec![microsoft_config("a", "A")]).unwrap();
        assert_eq!(svc.list_connectors(None).len(), 1);

        // Reload with a different set — should replace, not accumulate
        svc.load_connectors(vec![microsoft_config("b", "B"), microsoft_config("c", "C")]).unwrap();

        let list = svc.list_connectors(None);
        assert_eq!(list.len(), 2);
        assert!(list.iter().all(|c| c.id == "b" || c.id == "c"));
    }

    #[test]
    fn test_connectors_dir_getter() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("my-connectors");
        let svc = ConnectorService::new(&dir).unwrap();
        assert_eq!(svc.connectors_dir(), dir);
    }

    // ── list_connector_configs returns full config shape ─────────────

    #[test]
    fn test_list_connector_configs_returns_full_configs() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();

        // Empty before loading
        assert!(svc.list_connector_configs().is_empty());

        let cfgs =
            vec![microsoft_config("ms-work", "Work"), microsoft_config("ms-personal", "Personal")];
        svc.load_connectors(cfgs).unwrap();

        let configs = svc.list_connector_configs();
        assert_eq!(configs.len(), 2);

        for cfg in &configs {
            // Verify full ConnectorConfig fields are present
            assert!(!cfg.id.is_empty());
            assert!(!cfg.name.is_empty());
            assert_eq!(cfg.provider, ConnectorProvider::Microsoft);
            assert!(cfg.enabled);

            // Auth and services must be present (this is what the UI needs)
            matches!(cfg.auth, AuthConfig::OAuth2 { .. });
            let _ = &cfg.services;
        }

        // Serialized JSON must include "auth" and "services" keys — the API
        // sends this to the frontend which expects the full ConnectorConfig shape.
        let json_val = serde_json::to_value(&configs).unwrap();
        let arr = json_val.as_array().unwrap();
        for item in arr {
            assert!(item.get("id").is_some(), "missing 'id'");
            assert!(item.get("name").is_some(), "missing 'name'");
            assert!(item.get("provider").is_some(), "missing 'provider'");
            assert!(item.get("enabled").is_some(), "missing 'enabled'");
            assert!(item.get("auth").is_some(), "missing 'auth'");
            assert!(item.get("services").is_some(), "missing 'services'");

            // Auth type tag must be present
            let auth = item.get("auth").unwrap();
            assert_eq!(auth.get("type").and_then(|v| v.as_str()), Some("oauth2"));

            // Secrets must NOT appear in serialized output
            assert!(auth.get("refresh_token").is_none(), "refresh_token secret leaked");
            assert!(auth.get("client_secret").is_none(), "client_secret secret leaked");
            assert!(auth.get("access_token").is_none(), "access_token secret leaked");
        }
    }

    // ── Apple connector ──────────────────────────────────────────────

    fn apple_config(id: &str, name: &str) -> ConnectorConfig {
        ConnectorConfig {
            id: id.into(),
            name: name.into(),
            provider: ConnectorProvider::Apple,
            enabled: true,
            auth: AuthConfig::Local,
            services: ServicesConfig {
                communication: None,
                calendar: Some(CalendarConfig {
                    enabled: true,
                    default_class: DataClass::Internal,
                    resource_rules: vec![],
                }),
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
        }
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_apple_connector_creation() {
        let cfg = apple_config("apple-test", "My Apple");
        let connector = ConnectorService::try_create_connector(&cfg).unwrap();
        assert_eq!(connector.id(), "apple-test");
        assert_eq!(connector.provider(), ConnectorProvider::Apple);
        let services = connector.enabled_services();
        assert!(services.contains(&ServiceType::Calendar));
        assert!(services.contains(&ServiceType::Contacts));
    }

    #[test]
    #[cfg(not(target_os = "macos"))]
    fn test_apple_connector_fails_on_non_macos() {
        let cfg = apple_config("apple-test", "My Apple");
        let result = ConnectorService::try_create_connector(&cfg);
        let err = result.err().expect("expected error on non-macOS");
        assert!(err.to_string().contains("only available on macOS"), "unexpected error: {}", err);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn test_load_apple_connector() {
        let tmp = TempDir::new().unwrap();
        let svc = ConnectorService::new(tmp.path().join("c")).unwrap();
        svc.load_connectors(vec![apple_config("a1", "Apple Test")]).unwrap();
        let list = svc.list_connectors(None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, "a1");
        assert_eq!(list[0].provider, ConnectorProvider::Apple);
    }
}
