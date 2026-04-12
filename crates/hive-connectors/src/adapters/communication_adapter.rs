use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::Value;

use hive_classification::ChannelClass;
use hive_contracts::ToolApproval;

use crate::service_registry::{DynService, OperationSchema, ServiceDescriptor};
use crate::services::CommunicationService;

/// Wraps a [`CommunicationService`] into a [`DynService`].
pub struct CommunicationServiceAdapter {
    inner: Arc<dyn CommunicationService>,
}

impl CommunicationServiceAdapter {
    pub fn new(inner: Arc<dyn CommunicationService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DynService for CommunicationServiceAdapter {
    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            service_type: "communication".into(),
            display_name: self.inner.name().into(),
            description: "Email, messaging, and chat communication".into(),
            is_standard: true,
        }
    }

    fn operations(&self) -> Vec<OperationSchema> {
        vec![
            OperationSchema {
                name: "send".into(),
                description: "Send a message to one or more recipients".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "to": { "type": "array", "items": { "type": "string" } },
                        "subject": { "type": "string" },
                        "body": { "type": "string" }
                    },
                    "required": ["to", "body"]
                }),
                output_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "message_id": { "type": "string" } }
                })),
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Public,
            },
            OperationSchema {
                name: "fetch_new".into(),
                description: "Fetch new/unread messages".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "limit": { "type": "integer", "default": 10 }
                    }
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "mark_seen".into(),
                description: "Mark a message as seen/read".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message_id": { "type": "string" }
                    },
                    "required": ["message_id"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
            OperationSchema {
                name: "list_channels".into(),
                description: "List available channels, rooms, or folders".into(),
                input_schema: serde_json::json!({ "type": "object" }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Internal,
            },
        ]
    }

    async fn execute(&self, operation: &str, input: Value) -> anyhow::Result<Value> {
        match operation {
            "send" => {
                let to: Vec<String> = serde_json::from_value(
                    input.get("to").cloned().unwrap_or(Value::Array(vec![])),
                )?;
                let subject = input.get("subject").and_then(|v| v.as_str());
                let body = input.get("body").and_then(|v| v.as_str()).unwrap_or_default();
                let msg_id = self.inner.send(&to, subject, body, &[]).await?;
                Ok(serde_json::json!({ "message_id": msg_id }))
            }
            "fetch_new" => {
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                let messages = self.inner.fetch_new(limit).await?;
                let out: Vec<Value> = messages
                    .into_iter()
                    .map(|m| {
                        serde_json::json!({
                            "external_id": m.external_id,
                            "from": m.from,
                            "to": m.to,
                            "subject": m.subject,
                            "body": m.body,
                            "timestamp_ms": m.timestamp_ms,
                        })
                    })
                    .collect();
                Ok(Value::Array(out))
            }
            "mark_seen" => {
                let message_id =
                    input.get("message_id").and_then(|v| v.as_str()).unwrap_or_default();
                self.inner.mark_seen(message_id).await?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "list_channels" => {
                let channels = self.inner.list_channels().await?;
                Ok(serde_json::to_value(channels)?)
            }
            other => bail!("unknown communication operation: {other}"),
        }
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        self.inner.test_connection().await
    }

    fn as_communication(&self) -> Option<&dyn CommunicationService> {
        Some(self.inner.as_ref())
    }
}
