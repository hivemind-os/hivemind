use std::sync::Arc;

use anyhow::bail;
use async_trait::async_trait;
use serde_json::Value;

use hive_classification::ChannelClass;
use hive_contracts::connectors::{EventUpdate, NewCalendarEvent};
use hive_contracts::ToolApproval;

use crate::service_registry::{DynService, OperationSchema, ServiceDescriptor};
use crate::services::CalendarService;

/// Wraps a [`CalendarService`] into a [`DynService`].
pub struct CalendarServiceAdapter {
    inner: Arc<dyn CalendarService>,
}

impl CalendarServiceAdapter {
    pub fn new(inner: Arc<dyn CalendarService>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl DynService for CalendarServiceAdapter {
    fn descriptor(&self) -> ServiceDescriptor {
        ServiceDescriptor {
            service_type: "calendar".into(),
            display_name: self.inner.name().into(),
            description: "Calendar events and scheduling".into(),
            is_standard: true,
        }
    }

    fn operations(&self) -> Vec<OperationSchema> {
        vec![
            OperationSchema {
                name: "list_events".into(),
                description: "List calendar events within a time range".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "start": { "type": "string", "description": "ISO 8601 start time" },
                        "end": { "type": "string", "description": "ISO 8601 end time" }
                    },
                    "required": ["start", "end"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "get_event".into(),
                description: "Get a single calendar event by ID".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "event_id": { "type": "string" }
                    },
                    "required": ["event_id"]
                }),
                output_schema: None,
                side_effects: false,
                approval: ToolApproval::Auto,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "create_event".into(),
                description: "Create a new calendar event".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "subject": { "type": "string" },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "body": { "type": "string" },
                        "attendees": { "type": "array", "items": { "type": "string" } },
                        "location": { "type": "string" },
                        "is_online": { "type": "boolean" }
                    },
                    "required": ["subject", "start", "end"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "update_event".into(),
                description: "Update an existing calendar event".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "event_id": { "type": "string" },
                        "subject": { "type": "string" },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "body": { "type": "string" },
                        "location": { "type": "string" }
                    },
                    "required": ["event_id"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "delete_event".into(),
                description: "Delete a calendar event".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "event_id": { "type": "string" }
                    },
                    "required": ["event_id"]
                }),
                output_schema: None,
                side_effects: true,
                approval: ToolApproval::Ask,
                channel_class: ChannelClass::Private,
            },
            OperationSchema {
                name: "check_availability".into(),
                description: "Check free/busy availability within a time range".into(),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "start": { "type": "string" },
                        "end": { "type": "string" }
                    },
                    "required": ["start", "end"]
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
            "list_events" => {
                let start = input.get("start").and_then(|v| v.as_str()).unwrap_or_default();
                let end = input.get("end").and_then(|v| v.as_str()).unwrap_or_default();
                let events = self.inner.list_events(start, end).await?;
                Ok(serde_json::to_value(events)?)
            }
            "get_event" => {
                let event_id = input.get("event_id").and_then(|v| v.as_str()).unwrap_or_default();
                let event = self.inner.get_event(event_id).await?;
                Ok(serde_json::to_value(event)?)
            }
            "create_event" => {
                let event: NewCalendarEvent = serde_json::from_value(input)?;
                let created = self.inner.create_event(&event).await?;
                Ok(serde_json::to_value(created)?)
            }
            "update_event" => {
                let event_id = input.get("event_id").and_then(|v| v.as_str()).unwrap_or_default();
                let update: EventUpdate = serde_json::from_value(input.clone())?;
                let updated = self.inner.update_event(event_id, &update).await?;
                Ok(serde_json::to_value(updated)?)
            }
            "delete_event" => {
                let event_id = input.get("event_id").and_then(|v| v.as_str()).unwrap_or_default();
                self.inner.delete_event(event_id).await?;
                Ok(serde_json::json!({ "status": "ok" }))
            }
            "check_availability" => {
                let start = input.get("start").and_then(|v| v.as_str()).unwrap_or_default();
                let end = input.get("end").and_then(|v| v.as_str()).unwrap_or_default();
                let slots = self.inner.check_availability(start, end).await?;
                Ok(serde_json::to_value(slots)?)
            }
            other => bail!("unknown calendar operation: {other}"),
        }
    }

    async fn test_connection(&self) -> anyhow::Result<()> {
        self.inner.test_connection().await
    }

    fn as_calendar(&self) -> Option<&dyn CalendarService> {
        Some(self.inner.as_ref())
    }
}
