//! `Connector` implementation for Coinbase.

use std::sync::Arc;

use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

use crate::connector::Connector;
use crate::service_registry::ServiceRegistry;
use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

use super::trading::CoinbaseTradingService;

// ---------------------------------------------------------------------------
// CoinbaseConnector
// ---------------------------------------------------------------------------

pub struct CoinbaseConnector {
    id: String,
    name: String,
    trading: Option<Arc<CoinbaseTradingService>>,
    service_reg: ServiceRegistry,
}

impl CoinbaseConnector {
    pub fn new(id: &str, name: &str, trading: Option<CoinbaseTradingService>) -> Self {
        let trading = trading.map(Arc::new);
        let mut service_reg = ServiceRegistry::new();
        if let Some(ref t) = trading {
            service_reg.register(Arc::clone(t) as Arc<dyn crate::service_registry::DynService>);
        }
        Self { id: id.to_string(), name: name.to_string(), trading, service_reg }
    }
}

impl Connector for CoinbaseConnector {
    fn id(&self) -> &str {
        &self.id
    }

    fn display_name(&self) -> &str {
        &self.name
    }

    fn provider(&self) -> ConnectorProvider {
        ConnectorProvider::Coinbase
    }

    fn enabled_services(&self) -> Vec<ServiceType> {
        let mut s = vec![];
        if self.trading.is_some() {
            s.push(ServiceType::Other("trading".into()));
        }
        s
    }

    fn status(&self) -> ConnectorStatus {
        ConnectorStatus::Connected
    }

    fn communication(&self) -> Option<&dyn CommunicationService> {
        None
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::super::api::CoinbaseClient;
    use super::*;

    /// A test EC P-256 private key in PKCS#8 format (for unit tests only).
    const TEST_EC_PEM: &str = "-----BEGIN PRIVATE KEY-----\n\
MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgl0V43geGdE1aUifF\n\
Yl9SkTFxl51Pzhxf1ceo4TTX4x2hRANCAAS0d4TxS/dVRfp8uugFXbSD2oFKKxdz\n\
WFqar8wj03nITtVkHqWT5oLXHtnpcrCFMnCUrr7BH7gJwUpeGedSgKV/\n\
-----END PRIVATE KEY-----";

    fn test_connector(with_trading: bool) -> CoinbaseConnector {
        let trading = if with_trading {
            let client = Arc::new(
                CoinbaseClient::new("test", "orgs/1/apiKeys/2", TEST_EC_PEM, true).unwrap(),
            );
            Some(CoinbaseTradingService::new(client))
        } else {
            None
        };
        CoinbaseConnector::new("my-coinbase", "My Coinbase", trading)
    }

    #[test]
    fn provider_is_coinbase() {
        let c = test_connector(true);
        assert_eq!(c.provider(), ConnectorProvider::Coinbase);
    }

    #[test]
    fn enabled_services_with_trading() {
        let c = test_connector(true);
        let services = c.enabled_services();
        assert_eq!(services.len(), 1);
        assert_eq!(services[0], ServiceType::Other("trading".into()));
    }

    #[test]
    fn enabled_services_without_trading() {
        let c = test_connector(false);
        assert!(c.enabled_services().is_empty());
    }

    #[test]
    fn no_builtin_services() {
        let c = test_connector(true);
        assert!(c.communication().is_none());
        assert!(c.calendar().is_none());
        assert!(c.drive().is_none());
        assert!(c.contacts().is_none());
    }

    #[test]
    fn service_registry_has_trading() {
        let c = test_connector(true);
        let reg = c.service_registry().expect("should have registry");
        assert_eq!(reg.len(), 1);
        assert!(reg.get("trading").is_some());
    }

    #[test]
    fn service_registry_empty_without_trading() {
        let c = test_connector(false);
        let reg = c.service_registry().expect("should have registry");
        assert!(reg.is_empty());
    }
}
