use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use hive_classification::ChannelClass;
use hive_contracts::ToolApproval;

use crate::services::{CalendarService, CommunicationService, ContactsService, DriveService};

// ---------------------------------------------------------------------------
// ServiceDescriptor — metadata for a service exposed by a connector
// ---------------------------------------------------------------------------

/// Describes a service that a connector exposes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceDescriptor {
    /// Machine-readable key, e.g. "communication", "ticketing", "custom-crm".
    pub service_type: String,
    /// Human-readable name shown in tool descriptions and UI.
    pub display_name: String,
    /// Brief description of what this service provides.
    pub description: String,
    /// `true` for the four built-in service archetypes (communication,
    /// calendar, drive, contacts).  Dynamic tool synthesis skips standard
    /// services since they already have dedicated typed tools.
    pub is_standard: bool,
}

// ---------------------------------------------------------------------------
// OperationSchema — describes one callable operation within a service
// ---------------------------------------------------------------------------

/// Schema for a single operation within a [`DynService`].  Used to synthesize
/// [`ToolDefinition`](hive_contracts::ToolDefinition) entries at runtime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationSchema {
    /// Operation identifier, e.g. "send", "list_events", "create_ticket".
    pub name: String,
    /// Human-readable description of this operation.
    pub description: String,
    /// JSON Schema for the operation's input parameters.
    pub input_schema: Value,
    /// Optional JSON Schema for the operation's output.
    pub output_schema: Option<Value>,
    /// Whether this operation has side-effects.
    pub side_effects: bool,
    /// Approval policy for this operation.
    pub approval: ToolApproval,
    /// Channel classification for data flowing through this operation.
    pub channel_class: ChannelClass,
}

// ---------------------------------------------------------------------------
// DynService — the generic service trait
// ---------------------------------------------------------------------------

/// A dynamically-described service that a connector can expose.
///
/// This is the generic counterpart to the typed service traits
/// ([`CommunicationService`], [`CalendarService`], etc.).  Providers can
/// implement it directly for custom service types, or use an adapter to
/// wrap an existing typed service.
#[async_trait]
pub trait DynService: Send + Sync {
    /// Metadata about this service.
    fn descriptor(&self) -> ServiceDescriptor;

    /// The set of operations this service supports.
    fn operations(&self) -> Vec<OperationSchema>;

    /// Execute an operation by name with the given JSON input.
    async fn execute(&self, operation: &str, input: Value) -> anyhow::Result<Value>;

    /// Test that the service's connection / credentials are valid.
    async fn test_connection(&self) -> anyhow::Result<()>;

    // -- Optional downcast to known archetype traits --------------------------

    /// Downcast to the typed [`CommunicationService`], if this is a
    /// communication service adapter.
    fn as_communication(&self) -> Option<&dyn CommunicationService> {
        None
    }

    /// Downcast to the typed [`CalendarService`], if applicable.
    fn as_calendar(&self) -> Option<&dyn CalendarService> {
        None
    }

    /// Downcast to the typed [`DriveService`], if applicable.
    fn as_drive(&self) -> Option<&dyn DriveService> {
        None
    }

    /// Downcast to the typed [`ContactsService`], if applicable.
    fn as_contacts(&self) -> Option<&dyn ContactsService> {
        None
    }
}

// ---------------------------------------------------------------------------
// ServiceRegistry — per-connector collection of services
// ---------------------------------------------------------------------------

/// A registry of [`DynService`] instances keyed by service type.
///
/// Each [`Connector`](crate::connector::Connector) can optionally expose a
/// `ServiceRegistry`, making its capabilities discoverable at runtime.
pub struct ServiceRegistry {
    services: HashMap<String, Arc<dyn DynService>>,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self { services: HashMap::new() }
    }

    /// Register a service.  Uses the `service_type` from the service's
    /// descriptor as the key.  Overwrites any previous service with the
    /// same key.
    pub fn register(&mut self, svc: Arc<dyn DynService>) {
        let key = svc.descriptor().service_type.clone();
        self.services.insert(key, svc);
    }

    /// Look up a service by its type key.
    pub fn get(&self, service_type: &str) -> Option<Arc<dyn DynService>> {
        self.services.get(service_type).cloned()
    }

    /// List descriptors for all registered services.
    pub fn list(&self) -> Vec<ServiceDescriptor> {
        self.services.values().map(|s| s.descriptor()).collect()
    }

    /// Returns `true` if no services are registered.
    pub fn is_empty(&self) -> bool {
        self.services.is_empty()
    }

    /// Number of registered services.
    pub fn len(&self) -> usize {
        self.services.len()
    }
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal DynService implementation for testing.
    struct StubService {
        desc: ServiceDescriptor,
        ops: Vec<OperationSchema>,
    }

    impl StubService {
        fn new(service_type: &str, is_standard: bool) -> Self {
            Self {
                desc: ServiceDescriptor {
                    service_type: service_type.into(),
                    display_name: service_type.into(),
                    description: format!("Stub {service_type} service"),
                    is_standard,
                },
                ops: vec![OperationSchema {
                    name: "test_op".into(),
                    description: "A test operation".into(),
                    input_schema: serde_json::json!({"type": "object"}),
                    output_schema: None,
                    side_effects: false,
                    approval: ToolApproval::Auto,
                    channel_class: ChannelClass::Internal,
                }],
            }
        }
    }

    #[async_trait]
    impl DynService for StubService {
        fn descriptor(&self) -> ServiceDescriptor {
            self.desc.clone()
        }
        fn operations(&self) -> Vec<OperationSchema> {
            self.ops.clone()
        }
        async fn execute(&self, operation: &str, _input: Value) -> anyhow::Result<Value> {
            Ok(serde_json::json!({ "op": operation, "status": "ok" }))
        }
        async fn test_connection(&self) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn registry_new_is_empty() {
        let reg = ServiceRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.list().is_empty());
    }

    #[test]
    fn registry_register_and_get() {
        let mut reg = ServiceRegistry::new();
        reg.register(Arc::new(StubService::new("ticketing", false)));
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());

        let svc = reg.get("ticketing").expect("should find ticketing");
        assert_eq!(svc.descriptor().service_type, "ticketing");
        assert!(!svc.descriptor().is_standard);
    }

    #[test]
    fn registry_overwrites_duplicate_key() {
        let mut reg = ServiceRegistry::new();
        reg.register(Arc::new(StubService::new("communication", true)));
        reg.register(Arc::new(StubService::new("communication", true)));
        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn registry_get_missing_returns_none() {
        let reg = ServiceRegistry::new();
        assert!(reg.get("nonexistent").is_none());
    }

    #[test]
    fn registry_list_returns_all_descriptors() {
        let mut reg = ServiceRegistry::new();
        reg.register(Arc::new(StubService::new("communication", true)));
        reg.register(Arc::new(StubService::new("ticketing", false)));
        reg.register(Arc::new(StubService::new("calendar", true)));

        let descs = reg.list();
        assert_eq!(descs.len(), 3);
        let types: Vec<&str> = descs.iter().map(|d| d.service_type.as_str()).collect();
        assert!(types.contains(&"communication"));
        assert!(types.contains(&"ticketing"));
        assert!(types.contains(&"calendar"));
    }

    #[test]
    fn operations_schema_round_trips() {
        let op = OperationSchema {
            name: "send".into(),
            description: "Send a message".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "to": { "type": "string" }
                }
            }),
            output_schema: Some(serde_json::json!({"type": "object"})),
            side_effects: true,
            approval: ToolApproval::Ask,
            channel_class: ChannelClass::Public,
        };
        let json = serde_json::to_string(&op).unwrap();
        let back: OperationSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(back.name, "send");
        assert!(back.side_effects);
    }

    #[test]
    fn service_descriptor_round_trips() {
        let desc = ServiceDescriptor {
            service_type: "ticketing".into(),
            display_name: "Ticketing".into(),
            description: "Issue tracker integration".into(),
            is_standard: false,
        };
        let json = serde_json::to_string(&desc).unwrap();
        let back: ServiceDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(back.service_type, "ticketing");
        assert!(!back.is_standard);
    }

    #[tokio::test]
    async fn dyn_service_execute() {
        let svc = StubService::new("ticketing", false);
        let result = svc.execute("create_ticket", serde_json::json!({})).await.unwrap();
        assert_eq!(result["op"], "create_ticket");
        assert_eq!(result["status"], "ok");
    }

    #[tokio::test]
    async fn dyn_service_test_connection() {
        let svc = StubService::new("ticketing", false);
        svc.test_connection().await.unwrap();
    }

    #[test]
    fn dyn_service_archetype_downcasts_default_to_none() {
        let svc = StubService::new("ticketing", false);
        assert!(svc.as_communication().is_none());
        assert!(svc.as_calendar().is_none());
        assert!(svc.as_drive().is_none());
        assert!(svc.as_contacts().is_none());
    }
}
