use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::adapters::{
    CalendarServiceAdapter, CommunicationServiceAdapter, ContactsServiceAdapter,
    DriveServiceAdapter,
};
use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

use super::calendar::MicrosoftCalendar;
use super::communication::MicrosoftCommunication;
use super::contacts::MicrosoftContacts;
use super::drive::MicrosoftDrive;
use super::graph_client::GraphClient;

/// Microsoft 365 connector using Graph API.
pub struct MicrosoftConnector {
    id: String,
    name: String,
    graph: Arc<GraphClient>,
    communication: Option<Arc<MicrosoftCommunication>>,
    calendar: Option<Arc<MicrosoftCalendar>>,
    drive: Option<Arc<MicrosoftDrive>>,
    contacts: Option<Arc<MicrosoftContacts>>,
    enabled_services: Vec<ServiceType>,
    service_reg: ServiceRegistry,
}

impl MicrosoftConnector {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: &str,
        name: &str,
        graph: Arc<GraphClient>,
        communication: Option<MicrosoftCommunication>,
        calendar: Option<MicrosoftCalendar>,
        drive: Option<MicrosoftDrive>,
        contacts: Option<MicrosoftContacts>,
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
            graph,
            communication,
            calendar,
            drive,
            contacts,
            enabled_services,
            service_reg,
        }
    }

    pub fn graph(&self) -> &Arc<GraphClient> {
        &self.graph
    }
}

impl Connector for MicrosoftConnector {
    fn id(&self) -> &str {
        &self.id
    }
    fn display_name(&self) -> &str {
        &self.name
    }
    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Microsoft
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
