use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::adapters::{
    CalendarServiceAdapter, CommunicationServiceAdapter, ContactsServiceAdapter,
    DriveServiceAdapter,
};
use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

use super::calendar::GoogleCalendar;
use super::communication::GmailCommunication;
use super::contacts::GoogleContacts;
use super::drive::GoogleDrive;
use super::google_client::GoogleClient;

/// Google / Gmail connector supporting communication, calendar, drive and contacts.
pub struct GmailConnector {
    id: String,
    name: String,
    google: Arc<GoogleClient>,
    communication: Option<Arc<GmailCommunication>>,
    calendar: Option<Arc<GoogleCalendar>>,
    drive: Option<Arc<GoogleDrive>>,
    contacts: Option<Arc<GoogleContacts>>,
    enabled_services: Vec<ServiceType>,
    service_reg: ServiceRegistry,
}

impl GmailConnector {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &str,
        name: &str,
        google: Arc<GoogleClient>,
        communication: Option<GmailCommunication>,
        calendar: Option<GoogleCalendar>,
        drive: Option<GoogleDrive>,
        contacts: Option<GoogleContacts>,
        enabled_services: Vec<ServiceType>,
    ) -> Self {
        let communication = communication.map(Arc::new);
        let calendar = calendar.map(Arc::new);
        let drive = drive.map(Arc::new);
        let contacts = contacts.map(Arc::new);

        let mut service_reg = ServiceRegistry::new();
        if let Some(ref c) = communication {
            service_reg.register(Arc::new(CommunicationServiceAdapter::new(
                Arc::clone(c) as Arc<dyn CommunicationService>
            )));
        }
        if let Some(ref c) = calendar {
            service_reg.register(Arc::new(CalendarServiceAdapter::new(
                Arc::clone(c) as Arc<dyn CalendarService>
            )));
        }
        if let Some(ref d) = drive {
            service_reg.register(Arc::new(DriveServiceAdapter::new(
                Arc::clone(d) as Arc<dyn DriveService>
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
            google,
            communication,
            calendar,
            drive,
            contacts,
            enabled_services,
            service_reg,
        }
    }

    pub fn google(&self) -> &Arc<GoogleClient> {
        &self.google
    }
}

impl Connector for GmailConnector {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Gmail
    }
    fn enabled_services(&self) -> Vec<ServiceType> {
        self.enabled_services.clone()
    }
    fn status(&self) -> ConnectorStatus {
        ConnectorStatus::Connected
    }

    fn communication(&self) -> Option<&dyn CommunicationService> {
        self.communication.as_deref().map(|c| c as &dyn CommunicationService)
    }
    fn calendar(&self) -> Option<&dyn CalendarService> {
        self.calendar.as_deref().map(|c| c as &dyn CalendarService)
    }
    fn drive(&self) -> Option<&dyn DriveService> {
        self.drive.as_deref().map(|d| d as &dyn DriveService)
    }
    fn contacts(&self) -> Option<&dyn ContactsService> {
        self.contacts.as_deref().map(|c| c as &dyn ContactsService)
    }

    fn service_registry(&self) -> Option<&ServiceRegistry> {
        Some(&self.service_reg)
    }
}
