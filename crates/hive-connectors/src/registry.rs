use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::RwLock;

use crate::Connector;

/// Entry in the registry pairing a connector with its access control list.
struct RegistryEntry {
    connector: Arc<dyn Connector>,
    /// Persona IDs allowed to use this connector (empty = no access).
    allowed_personas: Vec<String>,
}

/// Thread-safe registry of active connector instances, keyed by connector ID.
#[derive(Clone)]
pub struct ConnectorRegistry {
    connectors: Arc<RwLock<HashMap<String, RegistryEntry>>>,
}

impl ConnectorRegistry {
    pub fn new() -> Self {
        Self { connectors: Arc::new(RwLock::new(HashMap::new())) }
    }

    /// Register a connector with its allowed persona list.
    pub fn register_with_personas(
        &self,
        connector: Arc<dyn Connector>,
        allowed_personas: Vec<String>,
    ) {
        let id = connector.id().to_string();
        self.connectors.write().insert(id, RegistryEntry { connector, allowed_personas });
    }

    /// Register a connector (no persona restrictions — backwards compat).
    pub fn register(&self, connector: Arc<dyn Connector>) {
        self.register_with_personas(connector, Vec::new());
    }

    /// Look up a connector by ID.
    pub fn get(&self, id: &str) -> Option<Arc<dyn Connector>> {
        self.connectors.read().get(id).map(|e| e.connector.clone())
    }

    /// List all registered connectors.
    pub fn list(&self) -> Vec<Arc<dyn Connector>> {
        self.connectors.read().values().map(|e| e.connector.clone()).collect()
    }

    /// List connectors that the given persona is allowed to access.
    pub fn list_for_persona(&self, persona_id: &str) -> Vec<Arc<dyn Connector>> {
        self.connectors
            .read()
            .values()
            .filter(|e| e.allowed_personas.iter().any(|p| p == "*" || p == persona_id))
            .map(|e| e.connector.clone())
            .collect()
    }

    /// Remove a connector by ID. Returns the removed connector, if any.
    pub fn remove(&self, id: &str) -> Option<Arc<dyn Connector>> {
        self.connectors.write().remove(id).map(|e| e.connector)
    }
}

impl Default for ConnectorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};
    use hive_contracts::connectors::{ConnectorProvider, ConnectorStatus, ServiceType};

    struct FakeConnector {
        id: String,
    }

    impl Connector for FakeConnector {
        fn id(&self) -> &str {
            &self.id
        }
        fn display_name(&self) -> &str {
            "Fake"
        }
        fn provider(&self) -> ConnectorProvider {
            ConnectorProvider::Microsoft
        }
        fn enabled_services(&self) -> Vec<ServiceType> {
            vec![]
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
    }

    #[test]
    fn register_and_get() {
        let reg = ConnectorRegistry::new();
        reg.register(Arc::new(FakeConnector { id: "c1".into() }));
        assert!(reg.get("c1").is_some());
        assert!(reg.get("c2").is_none());
    }

    #[test]
    fn list_and_remove() {
        let reg = ConnectorRegistry::new();
        reg.register(Arc::new(FakeConnector { id: "a".into() }));
        reg.register(Arc::new(FakeConnector { id: "b".into() }));
        assert_eq!(reg.list().len(), 2);
        reg.remove("a");
        assert_eq!(reg.list().len(), 1);
        assert!(reg.get("a").is_none());
    }
}
