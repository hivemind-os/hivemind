use std::sync::Arc;

use serde_json::{json, Value};
use tracing::warn;

use hive_classification::DataClass;
use hive_connectors::{ConnectorRegistry, ServiceDescriptor};
use hive_contracts::{ToolAnnotations, ToolDefinition};

use crate::{BoxFuture, Tool, ToolError, ToolRegistry, ToolResult};

// ---------------------------------------------------------------------------
// ConnectorBridgeTool — synthesizes a Tool from a DynService operation
// ---------------------------------------------------------------------------

/// A dynamically-synthesized [`Tool`] that bridges a
/// [`DynService`](hive_connectors::DynService) operation into the hivemind tool
/// system.
///
/// Analogous to [`McpBridgeTool`](crate::McpBridgeTool), but backed by a
/// connector service rather than an MCP server.
pub struct ConnectorBridgeTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    service_type: String,
    operation: String,
}

impl ConnectorBridgeTool {
    /// Create a bridge tool from a connector service's operation schema.
    pub fn new(
        registry: Arc<ConnectorRegistry>,
        service_type: String,
        op: &hive_connectors::OperationSchema,
    ) -> Self {
        let tool_id = format!("connector.{}.{}", service_type, op.name);
        let display = format!("Connector: {} ({})", op.name, service_type);

        Self {
            definition: ToolDefinition {
                id: tool_id,
                name: display.clone(),
                description: op.description.clone(),
                input_schema: inject_connector_id_param(&op.input_schema),
                output_schema: op.output_schema.clone(),
                channel_class: op.channel_class,
                side_effects: op.side_effects,
                approval: op.approval,
                annotations: ToolAnnotations {
                    title: display,
                    read_only_hint: Some(!op.side_effects),
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: Some(true),
                },
            },
            registry,
            service_type,
            operation: op.name.clone(),
        }
    }
}

impl Tool for ConnectorBridgeTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id =
                input.get("connector_id").and_then(|v| v.as_str()).ok_or_else(|| {
                    ToolError::InvalidInput("missing required field: connector_id".into())
                })?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector not found: {connector_id}"))
            })?;

            let service = connector.dyn_service(&self.service_type).ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "service '{}' not available on connector '{}'",
                    self.service_type, connector_id,
                ))
            })?;

            let output = service
                .execute(&self.operation, input)
                .await
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;

            let data_class = channel_class_to_data_class(self.definition.channel_class);

            Ok(ToolResult { output, data_class })
        })
    }
}

// ---------------------------------------------------------------------------
// Registration helper
// ---------------------------------------------------------------------------

/// Register tools for all **non-standard** services across connectors
/// in the registry.
///
/// Standard services (communication, calendar, drive, contacts) are skipped
/// because they already have dedicated typed tools (`comm.*`, `calendar.*`,
/// etc.).  Only custom/dynamic services get bridge tools synthesized here.
///
/// When `persona_id` is `Some`, only connectors whose `allowed_personas`
/// includes that persona (or `"*"`) are considered.  When `None`, all
/// connectors are included (backward-compat for workflows/schedulers).
pub fn register_connector_service_tools(
    tool_registry: &mut ToolRegistry,
    connector_registry: &Arc<ConnectorRegistry>,
    persona_id: Option<&str>,
) {
    let connectors = match persona_id {
        Some(pid) => connector_registry.list_for_persona(pid),
        None => connector_registry.list(),
    };
    for connector in connectors {
        let Some(service_reg) = connector.service_registry() else {
            continue;
        };

        for desc in service_reg.list() {
            if desc.is_standard {
                continue;
            }

            let Some(service) = connector.dyn_service(&desc.service_type) else {
                continue;
            };

            for op in service.operations() {
                let tool = ConnectorBridgeTool::new(
                    Arc::clone(connector_registry),
                    desc.service_type.clone(),
                    &op,
                );
                if let Err(e) = tool_registry.register(Arc::new(tool)) {
                    warn!(
                        connector_id = %connector.id(),
                        service_type = %desc.service_type,
                        operation = %op.name,
                        error = %e,
                        "failed to register connector bridge tool (possible duplicate)"
                    );
                }
            }
        }
    }
}

/// List all service descriptors across all connectors, for discovery / UI.
pub fn list_all_connector_services(
    connector_registry: &ConnectorRegistry,
) -> Vec<(String, ServiceDescriptor)> {
    let mut result = Vec::new();
    for connector in connector_registry.list() {
        let Some(service_reg) = connector.service_registry() else {
            continue;
        };
        for desc in service_reg.list() {
            result.push((connector.id().to_string(), desc));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Ensure the operation's input schema includes a `connector_id` parameter
/// so the bridge tool can route to the right connector.
fn inject_connector_id_param(schema: &Value) -> Value {
    let mut schema = schema.clone();
    if let Some(obj) = schema.as_object_mut() {
        // Add connector_id to properties
        if let Some(props) = obj.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.entry("connector_id").or_insert_with(|| {
                json!({
                    "type": "string",
                    "description": "ID of the connector to use"
                })
            });
        }
        // Add connector_id to required
        if let Some(required) = obj.get_mut("required").and_then(|r| r.as_array_mut()) {
            let cid = Value::String("connector_id".into());
            if !required.contains(&cid) {
                required.push(cid);
            }
        }
    }
    schema
}

fn channel_class_to_data_class(cc: hive_classification::ChannelClass) -> DataClass {
    match cc {
        hive_classification::ChannelClass::Public => DataClass::Public,
        hive_classification::ChannelClass::Internal => DataClass::Internal,
        hive_classification::ChannelClass::Private => DataClass::Confidential,
        hive_classification::ChannelClass::LocalOnly => DataClass::Restricted,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::ToolApproval;

    #[test]
    fn inject_connector_id_adds_to_schema() {
        let schema = json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" }
            },
            "required": ["query"]
        });

        let patched = inject_connector_id_param(&schema);
        let props = patched["properties"].as_object().unwrap();
        assert!(props.contains_key("connector_id"));

        let required: Vec<&str> =
            patched["required"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
        assert!(required.contains(&"connector_id"));
        assert!(required.contains(&"query"));
    }

    #[test]
    fn inject_connector_id_idempotent() {
        let schema = json!({
            "type": "object",
            "properties": {
                "connector_id": { "type": "string" }
            },
            "required": ["connector_id"]
        });

        let patched = inject_connector_id_param(&schema);
        let required = patched["required"].as_array().unwrap();
        let count = required.iter().filter(|v| v.as_str() == Some("connector_id")).count();
        assert_eq!(count, 1, "connector_id should not be duplicated");
    }

    #[test]
    fn tool_id_format() {
        let registry = Arc::new(ConnectorRegistry::new());
        let op = hive_connectors::OperationSchema {
            name: "create_ticket".into(),
            description: "Create a support ticket".into(),
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
            output_schema: None,
            side_effects: true,
            approval: ToolApproval::Ask,
            channel_class: hive_classification::ChannelClass::Internal,
        };

        let tool = ConnectorBridgeTool::new(registry, "ticketing".into(), &op);
        assert_eq!(tool.definition().id, "connector.ticketing.create_ticket");
        assert!(tool.definition().side_effects);
    }

    #[test]
    fn register_skips_standard_services() {
        let connector_registry = Arc::new(ConnectorRegistry::new());
        // With no connectors, registration should be a no-op
        let mut tool_registry = ToolRegistry::new();
        register_connector_service_tools(&mut tool_registry, &connector_registry, None);
        assert!(tool_registry.list_definitions().is_empty());
    }
}
