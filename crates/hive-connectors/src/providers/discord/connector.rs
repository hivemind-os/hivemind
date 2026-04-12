use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::adapters::CommunicationServiceAdapter;
use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

use super::communication::DiscordCommunication;

pub struct DiscordConnector {
    id: String,
    name: String,
    communication: Option<Arc<DiscordCommunication>>,
    service_reg: ServiceRegistry,
}

impl DiscordConnector {
    pub fn new(id: &str, name: &str, communication: Option<DiscordCommunication>) -> Self {
        let communication = communication.map(Arc::new);
        let mut service_reg = ServiceRegistry::new();
        if let Some(ref c) = communication {
            service_reg.register(Arc::new(CommunicationServiceAdapter::new(
                Arc::clone(c) as Arc<dyn CommunicationService>
            )));
        }
        Self { id: id.to_string(), name: name.to_string(), communication, service_reg }
    }
}

impl Connector for DiscordConnector {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Discord
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
