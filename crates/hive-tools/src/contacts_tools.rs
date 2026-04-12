use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_connectors::ConnectorRegistry;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};
use std::sync::Arc;

// ===========================================================================
// contacts.list
// ===========================================================================

pub struct ContactsListTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl ContactsListTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "contacts.list".to_string(),
                name: "List Contacts".to_string(),
                description: "List contacts from the specified connector. Supports pagination with limit and offset parameters.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the contacts provider (use connector.list to find connectors with has_contacts=true)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of contacts to return (default: 20)",
                            "default": 20
                        },
                        "offset": {
                            "type": "integer",
                            "description": "Number of contacts to skip for pagination (default: 0)",
                            "default": 0
                        }
                    },
                    "required": ["connector_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "contacts": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "email": { "type": "string" },
                                    "phone": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Contacts".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for ContactsListTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);

            let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let contacts_svc = connector.contacts().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support contacts"
                ))
            })?;

            let contacts = contacts_svc
                .list_contacts(limit as usize, offset as usize)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("contacts error: {e}")))?;

            let output = json!({ "contacts": serde_json::to_value(&contacts).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// contacts.search
// ===========================================================================

pub struct ContactsSearchTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl ContactsSearchTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "contacts.search".to_string(),
                name: "Search Contacts".to_string(),
                description: "Search for contacts by name, email, or other fields. Returns matching contacts with metadata.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the contacts provider (use connector.list to find connectors with has_contacts=true)"
                        },
                        "query": {
                            "type": "string",
                            "description": "Search query string"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of results to return (default: 20)",
                            "default": 20
                        }
                    },
                    "required": ["connector_id", "query"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "contacts": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "email": { "type": "string" },
                                    "phone": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Search Contacts".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for ContactsSearchTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let query = input
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `query`".into()))?;

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let contacts_svc = connector.contacts().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support contacts"
                ))
            })?;

            let contacts = contacts_svc
                .search_contacts(query, limit as usize)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("contacts error: {e}")))?;

            let output = json!({ "contacts": serde_json::to_value(&contacts).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// contacts.get
// ===========================================================================

pub struct ContactsGetTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl ContactsGetTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "contacts.get".to_string(),
                name: "Get Contact".to_string(),
                description: "Get a single contact by its ID. Returns full contact details including name, email, phone, and other metadata.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the contacts provider (use connector.list to find connectors with has_contacts=true)"
                        },
                        "contact_id": {
                            "type": "string",
                            "description": "The ID of the contact to retrieve"
                        }
                    },
                    "required": ["connector_id", "contact_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "name": { "type": "string" },
                        "email": { "type": "string" },
                        "phone": { "type": "string" },
                        "company": { "type": "string" },
                        "title": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Get Contact".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
        }
    }
}

impl Tool for ContactsGetTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let contact_id = input
                .get("contact_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `contact_id`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let contacts_svc = connector.contacts().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support contacts"
                ))
            })?;

            let contact = contacts_svc
                .get_contact(contact_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("contacts error: {e}")))?;

            let output = serde_json::to_value(&contact)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}
