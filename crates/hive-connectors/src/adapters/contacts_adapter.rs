use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::Value;

use hive_classification::ChannelClass;
use hive_contracts::ToolApproval;

use crate::service_registry::{DynService, OperationSchema, ServiceDescriptor};
use crate::services::ContactsService;

/// Wraps a [`ContactsService`] into a [`DynService`].
pub struct ContactsServiceAdapter {
    inner: Arc<dyn ContactsService>,
}

impl ContactsServiceAdapter {
    pub fn new(inner: Arc<dyn ContactsService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DynService for ContactsServiceAdapter {
    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            service_type: "contacts".into(),
            display_name: self.inner.name().into(),
            description: "Contact list management and lookup".into(),
            is_standard: true,
        }
    }

    fn operations(&self) -> Vec<OperationSchema> {
        vec![
            OperationSchema {
                name: "list_contacts".into(),
                description: "List contacts with pagination".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "default": 50 },
                        "offset": { "type": "integer", "default": 0 }
                    }
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "search_contacts".into(),
                description: "Search contacts by name, email, etc.".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string" },
                        "limit": { "type": "integer", "default": 20 }
                    },
                    "required": ["query"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "get_contact".into(),
                description: "Get a single contact by ID".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "contact_id": { "type": "string" }
                    },
                    "required": ["contact_id"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
        ]
    }

    async fn execute(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        match operation {
            "list_contacts" => {
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize;
                let offset = input.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                let contacts = self.inner.list_contacts(limit, offset).await?;
                Ok(serde_json::to_value(contacts)?)
            }
            "search_contacts" => {
                let query = input.get("query").and_then(|v| v.as_str()).unwrap_or_default();
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;
                let contacts = self.inner.search_contacts(query, limit).await?;
                Ok(serde_json::to_value(contacts)?)
            }
            "get_contact" => {
                let contact_id =
                    input.get("contact_id").and_then(|v| v.as_str()).unwrap_or_default();
                let contact = self.inner.get_contact(contact_id).await?;
                Ok(serde_json::to_value(contact)?)
            }
            other => bail!("unknown contacts operation: {other}"),
        }
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        self.inner.test_connection().await
    }

    fn as_contacts(&self) -> Option<&dyn ContactsService> {
        Some(self.inner.as_ref())
    }
}
