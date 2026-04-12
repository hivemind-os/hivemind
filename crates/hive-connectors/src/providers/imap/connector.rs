use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};
use parking_lot::Mutex;

use crate::adapters::CommunicationServiceAdapter;
use crate::config::{AuthConfig, CommunicationConfig};
use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};
use crate::MessageState;

use super::communication::ImapCommunication;

pub struct ImapConnector {
    id: String,
    name: String,
    communication: Option<Arc<ImapCommunication>>,
    service_reg: ServiceRegistry,
}

impl ImapConnector {
    pub fn new(id: &str, name: &str, communication: Option<ImapCommunication>) -> Self {
        let communication = communication.map(Arc::new);
        let mut service_reg = ServiceRegistry::new();
        if let Some(ref c) = communication {
            service_reg.register(Arc::new(CommunicationServiceAdapter::new(
                Arc::clone(c) as Arc<dyn CommunicationService>
            )));
        }
        Self { id: id.to_string(), name: name.to_string(), communication, service_reg }
    }

    /// Build an `ImapConnector` from the unified connector config.
    ///
    /// Requires `AuthConfig::Password` (the IMAP provider is for non-OAuth
    /// plain IMAP). A `CommunicationConfig` enables the communication service.
    pub fn from_config(
        id: &str,
        name: &str,
        auth: &AuthConfig,
        comm_cfg: Option<&CommunicationConfig>,
        state_dir: &Path,
    ) -> Result<Self> {
        let communication = match (auth, comm_cfg) {
            (
                AuthConfig::Password {
                    username,
                    password,
                    imap_host,
                    imap_port,
                    smtp_host,
                    smtp_port,
                    smtp_encryption,
                },
                Some(cfg),
            ) if cfg.enabled => {
                let state_path = state_dir.join(format!("imap_{id}.db"));
                let state = MessageState::open(&state_path)
                    .with_context(|| format!("opening message state for connector {id}"))?;

                let from_address = cfg.from_address.clone().unwrap_or_else(|| username.clone());

                Some(ImapCommunication::new(
                    id,
                    imap_host.clone(),
                    *imap_port,
                    smtp_host.clone(),
                    *smtp_port,
                    *smtp_encryption,
                    username.clone(),
                    password.clone(),
                    from_address,
                    cfg.folder.clone(),
                    Arc::new(Mutex::new(state)),
                ))
            }
            _ => None,
        };

        Ok(Self::new(id, name, communication))
    }
}

impl Connector for ImapConnector {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Imap
    }

    fn enabled_services(&self) -> Vec<ServiceType> {
        let mut s = vec![];
        if self.communication.is_some() {
            s.push(ServiceType::Communication);
        }
        s
    }

    fn status(&self) -> ConnectorStatus {
        ConnectorStatus::Connected
    }

    fn communication(&self) -> Option<&dyn CommunicationService> {
        self.communication.as_deref().map(|c| c as &dyn CommunicationService)
    }

    fn calendar(&self) -> Option<&dyn CalendarService> {
        None
    }

    fn drive(&self) -> Option<&dyn DriveService> {
        None
    }

    fn contacts(&self) -> Option<&dyn ContactsService> {
        None
    }

    fn service_registry(&self) -> Option<&ServiceRegistry> {
        Some(&self.service_reg)
    }
}
