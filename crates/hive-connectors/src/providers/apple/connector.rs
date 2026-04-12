//! Apple macOS connector (Calendar + Contacts via native frameworks).

use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::adapters::{CalendarServiceAdapter, ContactsServiceAdapter};
use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, ContactsService};

use super::calendar::AppleCalendar;
use super::contacts::AppleContacts;

/// Apple connector for local macOS Calendar and Contacts.
pub struct AppleConnector {
    id: String,
    name: String,
    calendar: Option<Arc<AppleCalendar>>,
    contacts: Option<Arc<AppleContacts>>,
    enabled_services: Vec<ServiceType>,
    service_reg: ServiceRegistry,
}

impl AppleConnector {
    pub fn new(
        id: &str,
        name: &str,
        calendar: Option<AppleCalendar>,
        contacts: Option<AppleContacts>,
        enabled_services: Vec<ServiceType>,
    ) -> Self {
        let calendar = calendar.map(Arc::new);
        let contacts = contacts.map(Arc::new);

        let mut service_reg = ServiceRegistry::new();
        if let Some(ref c) = calendar {
            service_reg.register(Arc::new(CalendarServiceAdapter::new(
                Arc::clone(c) as Arc<dyn CalendarService>
            )));
        }
        if let Some(ref c) = contacts {
            service_reg.register(Arc::new(ContactsServiceAdapter::new(
                Arc::clone(c) as Arc<dyn ContactsService>
            )));
        }

        Self {
            id: id.to_string(),
            name: name.to_string(),
            calendar,
            contacts,
            enabled_services,
            service_reg,
        }
    }
}

impl Connector for AppleConnector {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Apple
    }
    fn enabled_services(&self) -> Vec<ServiceType> {
        self.enabled_services.clone()
    }
    fn status(&self) -> ConnectorStatus {
        ConnectorStatus::Connected
    }

    fn communication(&self) -> Option<&dyn crate::services::CommunicationService> {
        None
    }
    fn calendar(&self) -> Option<&dyn CalendarService> {
        self.calendar.as_ref().map(|c| c.as_ref() as &dyn CalendarService)
    }
    fn drive(&self) -> Option<&dyn crate::services::DriveService> {
        None
    }
    fn contacts(&self) -> Option<&dyn ContactsService> {
        self.contacts.as_ref().map(|c| c.as_ref() as &dyn ContactsService)
    }

    fn service_registry(&self) -> Option<&ServiceRegistry> {
        Some(&self.service_reg)
    }
}
