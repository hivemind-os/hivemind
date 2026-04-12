use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_connectors::ConnectorRegistry;
use hive_contracts::connectors::{EventUpdate, NewCalendarEvent};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};
use std::sync::Arc;

// ===========================================================================
// calendar.list_events
// ===========================================================================

pub struct CalendarListEventsTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CalendarListEventsTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "calendar.list_events".to_string(),
                name: "List Calendar Events".to_string(),
                description: "List calendar events within a date range for a given connector. Returns event ID, title, start/end times, location, and attendees.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the calendar provider (use connector.list to find connectors with has_calendar=true)"
                        },
                        "start": {
                            "type": "string",
                            "description": "Start of the date range (ISO 8601 timestamp)"
                        },
                        "end": {
                            "type": "string",
                            "description": "End of the date range (ISO 8601 timestamp)"
                        }
                    },
                    "required": ["connector_id", "start", "end"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "events": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "title": { "type": "string" },
                                    "start": { "type": "string" },
                                    "end": { "type": "string" },
                                    "location": { "type": "string" },
                                    "description": { "type": "string" },
                                    "attendees": { "type": "array", "items": { "type": "string" } }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Calendar Events".to_string(),
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

impl Tool for CalendarListEventsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let start = input
                .get("start")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `start`".into()))?;

            let end = input
                .get("end")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `end`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let calendar = connector.calendar().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support calendar"
                ))
            })?;

            let events = calendar
                .list_events(start, end)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("calendar error: {e}")))?;

            let output = json!({ "events": serde_json::to_value(&events).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// calendar.create_event
// ===========================================================================

pub struct CalendarCreateEventTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CalendarCreateEventTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "calendar.create_event".to_string(),
                name: "Create Calendar Event".to_string(),
                description: "Create a new calendar event on the specified connector. Requires title, start, and end times. Optionally include description, location, attendees, and all-day flag.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the calendar provider (use connector.list to find connectors with has_calendar=true)"
                        },
                        "title": {
                            "type": "string",
                            "description": "Title of the event"
                        },
                        "start": {
                            "type": "string",
                            "description": "Start time (ISO 8601 timestamp)"
                        },
                        "end": {
                            "type": "string",
                            "description": "End time (ISO 8601 timestamp)"
                        },
                        "description": {
                            "type": "string",
                            "description": "Optional event description"
                        },
                        "location": {
                            "type": "string",
                            "description": "Optional event location"
                        },
                        "attendees": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Optional list of attendee email addresses"
                        },
                        "is_all_day": {
                            "type": "boolean",
                            "description": "Whether the event is an all-day event (default: false)"
                        },
                        "timezone": {
                            "type": "string",
                            "description": "Optional IANA timezone (e.g. 'America/New_York'). If omitted, the calendar's default timezone is used."
                        }
                    },
                    "required": ["connector_id", "title", "start", "end"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "event_id": { "type": "string" },
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Create Calendar Event".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                },
            },
            registry,
        }
    }
}

impl Tool for CalendarCreateEventTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let title = input
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `title`".into()))?;

            let start = input
                .get("start")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `start`".into()))?;

            let end = input
                .get("end")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `end`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let calendar = connector.calendar().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support calendar"
                ))
            })?;

            let event = NewCalendarEvent {
                title: title.to_string(),
                start: start.to_string(),
                end: end.to_string(),
                description: input.get("description").and_then(|v| v.as_str()).map(String::from),
                location: input.get("location").and_then(|v| v.as_str()).map(String::from),
                attendees: input
                    .get("attendees")
                    .and_then(|v| v.as_array())
                    .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                is_all_day: input.get("is_all_day").and_then(|v| v.as_bool()).unwrap_or(false),
                timezone: input.get("timezone").and_then(|v| v.as_str()).map(String::from),
            };

            let created = calendar
                .create_event(&event)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("calendar error: {e}")))?;

            let output = serde_json::to_value(&created)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// calendar.update_event
// ===========================================================================

pub struct CalendarUpdateEventTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CalendarUpdateEventTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "calendar.update_event".to_string(),
                name: "Update Calendar Event".to_string(),
                description: "Update an existing calendar event. Requires connector_id and event_id. All other fields are optional and only provided fields will be updated.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the calendar provider (use connector.list to find connectors with has_calendar=true)"
                        },
                        "event_id": {
                            "type": "string",
                            "description": "The ID of the event to update"
                        },
                        "title": {
                            "type": "string",
                            "description": "New title for the event"
                        },
                        "start": {
                            "type": "string",
                            "description": "New start time (ISO 8601 timestamp)"
                        },
                        "end": {
                            "type": "string",
                            "description": "New end time (ISO 8601 timestamp)"
                        },
                        "description": {
                            "type": "string",
                            "description": "New description for the event"
                        },
                        "location": {
                            "type": "string",
                            "description": "New location for the event"
                        },
                        "timezone": {
                            "type": "string",
                            "description": "Optional IANA timezone for start/end (e.g. 'America/New_York'). If omitted, the calendar's default timezone is used."
                        }
                    },
                    "required": ["connector_id", "event_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "event_id": { "type": "string" },
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Update Calendar Event".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                },
            },
            registry,
        }
    }
}

impl Tool for CalendarUpdateEventTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let event_id = input
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `event_id`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let calendar = connector.calendar().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support calendar"
                ))
            })?;

            let update = EventUpdate {
                title: input.get("title").and_then(|v| v.as_str()).map(String::from),
                start: input.get("start").and_then(|v| v.as_str()).map(String::from),
                end: input.get("end").and_then(|v| v.as_str()).map(String::from),
                description: input.get("description").and_then(|v| v.as_str()).map(String::from),
                location: input.get("location").and_then(|v| v.as_str()).map(String::from),
                timezone: input.get("timezone").and_then(|v| v.as_str()).map(String::from),
            };

            let updated = calendar
                .update_event(event_id, &update)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("calendar error: {e}")))?;

            let output = serde_json::to_value(&updated)
                .map_err(|e| ToolError::ExecutionFailed(e.to_string()))?;
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// calendar.delete_event
// ===========================================================================

pub struct CalendarDeleteEventTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CalendarDeleteEventTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "calendar.delete_event".to_string(),
                name: "Delete Calendar Event".to_string(),
                description: "Delete a calendar event by its ID. This action is destructive and cannot be undone.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the calendar provider (use connector.list to find connectors with has_calendar=true)"
                        },
                        "event_id": {
                            "type": "string",
                            "description": "The ID of the event to delete"
                        }
                    },
                    "required": ["connector_id", "event_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "status": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Delete Calendar Event".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(true),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                },
            },
            registry,
        }
    }
}

impl Tool for CalendarDeleteEventTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let event_id = input
                .get("event_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `event_id`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let calendar = connector.calendar().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support calendar"
                ))
            })?;

            calendar
                .delete_event(event_id)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("calendar error: {e}")))?;

            Ok(ToolResult { output: json!({ "deleted": true }), data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// calendar.check_availability
// ===========================================================================

pub struct CalendarCheckAvailabilityTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CalendarCheckAvailabilityTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "calendar.check_availability".to_string(),
                name: "Check Calendar Availability".to_string(),
                description: "Check free/busy availability for a given time range. Returns busy slots within the specified window.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID for the calendar provider (use connector.list to find connectors with has_calendar=true)"
                        },
                        "start": {
                            "type": "string",
                            "description": "Start of the availability window (ISO 8601 timestamp)"
                        },
                        "end": {
                            "type": "string",
                            "description": "End of the availability window (ISO 8601 timestamp)"
                        }
                    },
                    "required": ["connector_id", "start", "end"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "busy": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "start": { "type": "string" },
                                    "end": { "type": "string" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Check Calendar Availability".to_string(),
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

impl Tool for CalendarCheckAvailabilityTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let start = input
                .get("start")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `start`".into()))?;

            let end = input
                .get("end")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `end`".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let calendar = connector.calendar().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support calendar"
                ))
            })?;

            let slots = calendar
                .check_availability(start, end)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("calendar error: {e}")))?;

            let output = json!({ "slots": serde_json::to_value(&slots).map_err(|e| ToolError::ExecutionFailed(e.to_string()))? });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}
