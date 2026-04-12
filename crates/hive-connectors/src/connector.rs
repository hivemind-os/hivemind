use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::service_registry::{DynService, ServiceRegistry};
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

// Re-export InboundMessage from the communication service.
pub use crate::services::communication::InboundMessage;

/// A connector is an authenticated connection to a provider that can expose
/// one or more services (communication, calendar, drive, contacts).
pub trait Connector: Send + Sync {
    /// Unique connector ID (user-chosen, e.g. "work-microsoft").
    fn id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> &str;

    /// Which provider this connector is backed by.
    fn provider(&self) -> ConnectorProvider;

    /// The set of service types this connector *can* provide (based on provider).
    fn available_services(&self) -> &[ServiceType] {
        self.provider().available_services()
    }

    /// The set of service types currently enabled by the user.
    fn enabled_services(&self) -> Vec<ServiceType>;

    /// Current connection status.
    fn status(&self) -> ConnectorStatus;

    /// Access the Communication service, if supported and enabled.
    fn communication(&self) -> Option<&dyn CommunicationService>;

    /// Access the Calendar service, if supported and enabled.
    fn calendar(&self) -> Option<&dyn CalendarService>;

    /// Access the Drive service, if supported and enabled.
    fn drive(&self) -> Option<&dyn DriveService>;

    /// Access the Contacts service, if supported and enabled.
    fn contacts(&self) -> Option<&dyn ContactsService>;

    // -- Generic service registry (opt-in) ------------------------------------

    /// Access the dynamic service registry for this connector.
    ///
    /// Returns `None` by default.  Providers that support the generic
    /// service model override this to expose their services via
    /// [`ServiceRegistry`].
    fn service_registry(&self) -> Option<&ServiceRegistry> {
        None
    }

    /// Look up a single dynamic service by its type key
    /// (e.g. `"communication"`, `"ticketing"`).
    ///
    /// Default implementation delegates to [`service_registry`](Self::service_registry).
    fn dyn_service(&self, service_type: &str) -> Option<Arc<dyn DynService>> {
        self.service_registry()?.get(service_type)
    }
}
