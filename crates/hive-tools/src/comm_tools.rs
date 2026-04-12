use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_connectors::audit::{AuditStore, ConnectorAuditFilter};
use hive_connectors::services::communication::CommAttachment;
use hive_connectors::{ConnectorAuditLog, ConnectorRegistry, ConnectorServiceHandle};
use hive_contracts::comms::MessageDirection;
use hive_contracts::connectors::ServiceType;
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use serde_json::{json, Value};
use std::sync::Arc;

// ===========================================================================
// connector.list
// ===========================================================================

pub struct ListConnectorsTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    persona_id: String,
}

impl ListConnectorsTool {
    pub fn new(registry: Arc<ConnectorRegistry>, persona_id: String) -> Self {
        Self {
            definition: ToolDefinition {
                id: "connector.list".to_string(),
                name: "List Connectors".to_string(),
                description: "List available connectors and their capabilities. Returns connector ID, name, type, connectivity status, and which services are available (communication, calendar, drive, contacts). Use this to discover connector IDs for calendar, drive, and contacts tools.".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "required": []
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "connectors": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "name": { "type": "string" },
                                    "connectorType": { "type": "string" },
                                    "enabled": { "type": "boolean" },
                                    "status": { "type": "object" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Connectors".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            registry,
            persona_id,
        }
    }
}

impl Tool for ListConnectorsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, _input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connectors: Vec<Value> = self
                .registry
                .list_for_persona(&self.persona_id)
                .into_iter()
                .map(|c| {
                    json!({
                        "id": c.id(),
                        "name": c.display_name(),
                        "connectorType": format!("{:?}", c.provider()),
                        "enabled": true,
                        "has_communication": c.communication().is_some(),
                        "has_calendar": c.calendar().is_some(),
                        "has_drive": c.drive().is_some(),
                        "has_contacts": c.contacts().is_some(),
                        "status": serde_json::to_value(c.status()).unwrap_or(json!("unknown")),
                    })
                })
                .collect();

            Ok(ToolResult {
                output: json!({ "connectors": connectors }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ===========================================================================
// comm.list_channels
// ===========================================================================

pub struct CommListChannelsTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CommListChannelsTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "comm.list_channels".to_string(),
                name: "List Channels for Connector".to_string(),
                description: concat!(
                    "List the channels, rooms, or folders available on a specific communication connector. ",
                    "For Discord this returns guild channels, for Slack workspace channels, ",
                    "for email providers mailbox folders. ",
                    "Use connector.list first to discover available connector IDs."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID to list channels for (from connector.list)"
                        }
                    },
                    "required": ["connector_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "channels": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string", "description": "Channel/folder ID to use with send_external_message" },
                                    "name": { "type": "string", "description": "Human-readable channel name" },
                                    "channel_type": { "type": "string", "description": "e.g. text, voice, dm, folder, group" },
                                    "group_name": { "type": "string", "description": "Parent group (Discord guild, Slack workspace)" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "List Channels for Connector".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                },
            },
            registry,
        }
    }
}

impl Tool for CommListChannelsTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("connector_id is required".into()))?;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::InvalidInput(format!("connector '{connector_id}' not found"))
            })?;

            let comm = connector.communication().ok_or_else(|| {
                ToolError::InvalidInput(format!(
                    "connector '{connector_id}' does not support communication"
                ))
            })?;

            let channels = comm.list_channels().await.map_err(|e| {
                ToolError::ExecutionFailed(format!("failed to list channels: {e:#}"))
            })?;

            let items: Vec<Value> =
                channels.iter().map(|ch| serde_json::to_value(ch).unwrap_or(json!({}))).collect();

            Ok(ToolResult { output: json!({ "channels": items }), data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// comm.send_external_message
// ===========================================================================

pub struct CommSendMessageTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    connector_service: Option<Arc<dyn ConnectorServiceHandle>>,
    default_dir: Option<std::path::PathBuf>,
}

impl CommSendMessageTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self::with_service(registry, None, None)
    }

    pub fn with_service(
        registry: Arc<ConnectorRegistry>,
        connector_service: Option<Arc<dyn ConnectorServiceHandle>>,
        default_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "comm.send_external_message".to_string(),
                name: "Send External Message".to_string(),
                description: concat!(
                    "Send an external message through a communication connector (email, Slack, Discord, etc.). ",
                    "Requires the connector_id (use connector.list to discover available connectors), ",
                    "a destination address (email address, Slack channel, etc.), and the message body. ",
                    "Subject is optional. Supports file attachments via the attachments array. ",
                    "The message will be checked against the connector's data classification ",
                    "and approval rules before sending. ",
                    "This tool is ONLY for sending external communications through a configured connector. ",
                    "Do NOT use this for internal agent-to-agent signaling (use core.signal_agent instead) ",
                    "or for asking the interactive user a question (use core.ask_user instead)."
                ).to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID to send through (from connector.list)"
                        },
                        "to": {
                            "type": "string",
                            "description": "Destination address. For email: the recipient email address. For Discord/Slack: the channel ID to post in (use the channel ID from the incoming message's `to` field or `metadata.channel_id`). Must not be empty."
                        },
                        "subject": {
                            "type": "string",
                            "description": "Optional message subject (used for email)"
                        },
                        "body": {
                            "type": "string",
                            "description": "The message body to send"
                        },
                        "attachments": {
                            "type": "array",
                            "description": "Optional file attachments. Each item specifies a workspace-relative file path and an optional filename override.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "path": { "type": "string", "description": "Relative path to the file in the workspace." },
                                    "filename": { "type": "string", "description": "Override filename (defaults to the file's basename)." }
                                },
                                "required": ["path"]
                            }
                        }
                    },
                    "required": ["connector_id", "to", "body"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "message_id": { "type": "string" },
                        "status": { "type": "string" },
                        "data_class": { "type": "string" },
                        "attachments_sent": { "type": "number" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Send External Message".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(false),
                    open_world_hint: Some(true),
                },
            },
            registry,
            connector_service,
            default_dir,
        }
    }
}

impl Tool for CommSendMessageTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let to = input
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `to`".into()))?;

            if to.trim().is_empty() {
                return Err(ToolError::InvalidInput(
                    "`to` must not be empty. For Discord/Slack, use the channel ID from the incoming message.".into(),
                ));
            }

            let body = input
                .get("body")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `body`".into()))?;

            let subject = input.get("subject").and_then(|v| v.as_str());

            // Parse attachments from input
            let mut comm_attachments = Vec::new();
            if let Some(att_array) = input.get("attachments").and_then(|v| v.as_array()) {
                for att_val in att_array {
                    let path_str =
                        att_val.get("path").and_then(|v| v.as_str()).ok_or_else(|| {
                            ToolError::InvalidInput(
                                "each attachment must have a `path` field".into(),
                            )
                        })?;

                    let file_path = if let Some(ref root) = self.default_dir {
                        crate::resolve_existing_path(root, path_str)?
                    } else {
                        let p = std::path::Path::new(path_str).to_path_buf();
                        if !p.exists() {
                            return Err(ToolError::ExecutionFailed(format!(
                                "attachment file not found: {path_str}"
                            )));
                        }
                        p
                    };

                    let data = std::fs::read(&file_path).map_err(|e| {
                        ToolError::ExecutionFailed(format!(
                            "unable to read attachment '{path_str}': {e}"
                        ))
                    })?;

                    let filename = att_val
                        .get("filename")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| {
                            file_path
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| "file".to_string())
                        });

                    let media_type =
                        mime_guess::from_path(file_path).first_or_octet_stream().to_string();

                    comm_attachments.push(CommAttachment { id: None, filename, media_type, data });
                }
            }

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            // Enforce connector destination rules before sending.
            if let Some(ref svc) = self.connector_service {
                if let Some(approval) = svc.resolve_destination_approval(connector_id, to) {
                    tracing::info!(
                        connector_id,
                        destination = to,
                        approval = ?approval,
                        "connector destination rule check"
                    );
                    if approval == hive_contracts::ToolApproval::Deny {
                        return Err(ToolError::ExecutionFailed(format!(
                            "destination '{to}' is denied by a connector rule on '{connector_id}'"
                        )));
                    }
                }
            }

            let comm = connector.communication().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support communication"
                ))
            })?;

            let attachments_count = comm_attachments.len();
            let message_id =
                comm.send(&[to.to_string()], subject, body, &comm_attachments)
                    .await
                    .map_err(|e| ToolError::ExecutionFailed(format!("send error: {e:#}")))?;

            Ok(ToolResult {
                output: json!({
                    "message_id": message_id,
                    "status": "sent",
                    "attachments_sent": attachments_count,
                }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ===========================================================================
// comm.read_messages
// ===========================================================================

pub struct CommReadMessagesTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
}

impl CommReadMessagesTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "comm.read_messages".to_string(),
                name: "Read Messages".to_string(),
                description: concat!(
                    "Read new/unread messages from a communication connector. ",
                    "Returns messages that haven't been seen yet. ",
                    "Use connector.list to discover available connectors."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID to read from"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum number of messages to return (default: 20)",
                            "default": 20
                        }
                    },
                    "required": ["connector_id"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "messages": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "from": { "type": "string", "description": "Sender address or username" },
                                    "to": { "type": "array", "description": "For Discord/Slack: channel ID(s) where the message was posted. Use this value as the `to` parameter when replying with comm.send_external_message." },
                                    "subject": { "type": "string" },
                                    "body": { "type": "string" },
                                    "timestamp_ms": { "type": "integer" },
                                    "data_class": { "type": "string" },
                                    "metadata": { "type": "object", "description": "Additional message metadata (e.g. channel_id, guild_id for Discord)" },
                                    "attachments": {
                                        "type": "array",
                                        "description": "File attachments on this message",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "filename": { "type": "string" },
                                                "media_type": { "type": "string" },
                                                "size": { "type": "integer" }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Read Messages".to_string(),
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

impl Tool for CommReadMessagesTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20) as usize;

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let comm = connector.communication().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support communication"
                ))
            })?;

            let messages = comm
                .fetch_new(limit)
                .await
                .map_err(|e| ToolError::ExecutionFailed(format!("read error: {e:#}")))?;

            let msgs: Vec<Value> = messages
                .into_iter()
                .map(|m| {
                    let att_info: Vec<Value> = m
                        .attachments
                        .iter()
                        .map(|a| {
                            json!({
                                "id": a.id,
                                "filename": a.filename,
                                "media_type": a.media_type,
                                "size": a.data.len(),
                            })
                        })
                        .collect();
                    json!({
                        "id": m.external_id,
                        "from": m.from,
                        "to": m.to,
                        "subject": m.subject,
                        "body": m.body,
                        "timestamp_ms": m.timestamp_ms,
                        "metadata": m.metadata,
                        "attachments": att_info,
                    })
                })
                .collect();

            Ok(ToolResult { output: json!({ "messages": msgs }), data_class: DataClass::Internal })
        })
    }
}

// ===========================================================================
// comm.search_messages
// ===========================================================================

pub struct CommSearchMessagesTool {
    definition: ToolDefinition,
    audit_log: Arc<ConnectorAuditLog>,
}

impl CommSearchMessagesTool {
    pub fn new(audit_log: Arc<ConnectorAuditLog>) -> Self {
        Self {
            definition: ToolDefinition {
                id: "comm.search_messages".to_string(),
                name: "Search Messages".to_string(),
                description: concat!(
                    "Search the communication audit log for past messages. ",
                    "Filter by connector, sender, recipient, date range, or direction. ",
                    "Returns message metadata and preview."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "Filter by connector ID (optional)"
                        },
                        "direction": {
                            "type": "string",
                            "enum": ["inbound", "outbound"],
                            "description": "Filter by message direction (optional)"
                        },
                        "since": {
                            "type": "string",
                            "description": "ISO 8601 timestamp to filter messages after this time (optional)"
                        },
                        "until": {
                            "type": "string",
                            "description": "ISO 8601 timestamp to filter messages before this time (optional)"
                        },
                        "limit": {
                            "type": "integer",
                            "description": "Maximum results to return (default: 50)",
                            "default": 50
                        }
                    },
                    "required": []
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "results": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "connector_id": { "type": "string" },
                                    "direction": { "type": "string" },
                                    "from": { "type": "string" },
                                    "to": { "type": "string" },
                                    "subject": { "type": "string" },
                                    "body_preview": { "type": "string" },
                                    "timestamp_ms": { "type": "integer" }
                                }
                            }
                        }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: "Search Messages".to_string(),
                    read_only_hint: Some(true),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(false),
                },
            },
            audit_log,
        }
    }
}

impl Tool for CommSearchMessagesTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let direction = match input.get("direction").and_then(|v| v.as_str()) {
                Some("inbound") => Some(MessageDirection::Inbound),
                Some("outbound") => Some(MessageDirection::Outbound),
                _ => None,
            };

            let since_ms = input
                .get("since")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis() as u128);

            let until_ms = input
                .get("until")
                .and_then(|v| v.as_str())
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp_millis() as u128);

            let filter = ConnectorAuditFilter {
                connector_id: input.get("connector_id").and_then(|v| v.as_str()).map(String::from),
                service_type: Some(ServiceType::Communication),
                direction,
                agent_id: None,
                since_ms,
                until_ms,
                limit: Some(input.get("limit").and_then(|v| v.as_u64()).unwrap_or(50) as usize),
            };

            let entries = self
                .audit_log
                .query(&filter)
                .map_err(|e| ToolError::ExecutionFailed(format!("audit query error: {e:#}")))?;

            let results: Vec<Value> = entries
                .into_iter()
                .map(|e| {
                    json!({
                        "id": e.id,
                        "connector_id": e.connector_id,
                        "direction": e.direction.map(|d| d.to_string()),
                        "from": e.from_address,
                        "to": e.to_address,
                        "subject": e.subject,
                        "body_preview": e.body_preview,
                        "timestamp_ms": e.timestamp_ms,
                    })
                })
                .collect();

            Ok(ToolResult {
                output: json!({ "results": results }),
                data_class: DataClass::Internal,
            })
        })
    }
}

// ===========================================================================
// comm.download_attachment
// ===========================================================================

pub struct CommDownloadAttachmentTool {
    definition: ToolDefinition,
    registry: Arc<ConnectorRegistry>,
    default_dir: Option<std::path::PathBuf>,
}

impl CommDownloadAttachmentTool {
    pub fn new(registry: Arc<ConnectorRegistry>) -> Self {
        Self::with_workspace(registry, None)
    }

    pub fn with_workspace(
        registry: Arc<ConnectorRegistry>,
        default_dir: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            definition: ToolDefinition {
                id: "comm.download_attachment".to_string(),
                name: "Download Attachment".to_string(),
                description: concat!(
                    "Download an email attachment to a local file. ",
                    "Use the attachment ID from comm.read_messages output. ",
                    "The local_path must be a workspace-relative path."
                )
                .to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "connector_id": {
                            "type": "string",
                            "description": "The connector ID (from connector.list)"
                        },
                        "message_id": {
                            "type": "string",
                            "description": "The message external_id from comm.read_messages"
                        },
                        "attachment_id": {
                            "type": "string",
                            "description": "The attachment ID from the attachment metadata in comm.read_messages"
                        },
                        "local_path": {
                            "type": "string",
                            "description": "Workspace-relative path where the attachment should be saved (e.g. 'attachments/report.pdf')"
                        }
                    },
                    "required": ["connector_id", "message_id", "attachment_id", "local_path"]
                }),
                output_schema: Some(json!({
                    "type": "object",
                    "properties": {
                        "filename": { "type": "string" },
                        "media_type": { "type": "string" },
                        "size_bytes": { "type": "number" },
                        "local_path": { "type": "string" }
                    }
                })),
                channel_class: ChannelClass::Internal,
                side_effects: true,
                approval: ToolApproval::Ask,
                annotations: ToolAnnotations {
                    title: "Download Attachment".to_string(),
                    read_only_hint: Some(false),
                    destructive_hint: Some(false),
                    idempotent_hint: Some(true),
                    open_world_hint: Some(true),
                },
            },
            registry,
            default_dir,
        }
    }
}

impl Tool for CommDownloadAttachmentTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let connector_id = input
                .get("connector_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `connector_id`".into()))?;

            let message_id = input
                .get("message_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `message_id`".into()))?;

            let attachment_id = input
                .get("attachment_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `attachment_id`".into()))?;

            let local_path_str = input
                .get("local_path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| ToolError::InvalidInput("missing `local_path`".into()))?;

            // Validate local_path is within the workspace
            let resolved_path = if let Some(ref root) = self.default_dir {
                crate::resolve_relative_path(root, local_path_str)?
            } else {
                std::path::PathBuf::from(local_path_str)
            };

            let connector = self.registry.get(connector_id).ok_or_else(|| {
                ToolError::ExecutionFailed(format!("connector '{connector_id}' not found"))
            })?;

            let comm = connector.communication().ok_or_else(|| {
                ToolError::ExecutionFailed(format!(
                    "connector '{connector_id}' does not support communication"
                ))
            })?;

            let attachment =
                comm.download_attachment(message_id, attachment_id).await.map_err(|e| {
                    ToolError::ExecutionFailed(format!("download attachment error: {e:#}"))
                })?;

            // Create parent directories if needed and write to disk
            if let Some(parent) = resolved_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ToolError::ExecutionFailed(format!(
                        "failed to create directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
            std::fs::write(&resolved_path, &attachment.data).map_err(|e| {
                ToolError::ExecutionFailed(format!(
                    "failed to write file '{}': {e}",
                    resolved_path.display()
                ))
            })?;

            let output = json!({
                "filename": attachment.filename,
                "media_type": attachment.media_type,
                "size_bytes": attachment.data.len(),
                "local_path": resolved_path.to_string_lossy(),
            });
            Ok(ToolResult { output, data_class: DataClass::Internal })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;

    fn empty_registry() -> Arc<ConnectorRegistry> {
        Arc::new(ConnectorRegistry::new())
    }

    #[test]
    fn download_attachment_rejects_absolute_path() {
        let tool = CommDownloadAttachmentTool::with_workspace(
            empty_registry(),
            Some(std::env::temp_dir()),
        );
        let input = json!({
            "connector_id": "test",
            "message_id": "msg-1",
            "attachment_id": "att-1",
            "local_path": "/etc/evil.txt"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("absolute paths are not allowed"),
            "expected absolute path rejection, got: {err}"
        );
    }

    #[test]
    fn download_attachment_rejects_parent_traversal() {
        let tool = CommDownloadAttachmentTool::with_workspace(
            empty_registry(),
            Some(std::env::temp_dir()),
        );
        let input = json!({
            "connector_id": "test",
            "message_id": "msg-1",
            "attachment_id": "att-1",
            "local_path": "../../etc/shadow"
        });
        let result = tokio::runtime::Runtime::new().unwrap().block_on(tool.execute(input));
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("parent segments"),
            "expected parent traversal rejection, got: {err}"
        );
    }

    #[test]
    fn download_attachment_tool_definition() {
        let tool = CommDownloadAttachmentTool::new(empty_registry());
        let def = tool.definition();
        assert_eq!(def.id, "comm.download_attachment");
        let schema = &def.input_schema;
        let required = schema["required"].as_array().unwrap();
        let req_strs: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(req_strs.contains(&"connector_id"));
        assert!(req_strs.contains(&"message_id"));
        assert!(req_strs.contains(&"attachment_id"));
        assert!(req_strs.contains(&"local_path"));
    }

    #[test]
    fn send_message_tool_has_default_dir() {
        let tool = CommSendMessageTool::with_service(
            empty_registry(),
            None,
            Some(std::path::PathBuf::from("/workspace")),
        );
        assert!(tool.default_dir.is_some());
        assert_eq!(tool.default_dir.as_deref(), Some(std::path::Path::new("/workspace")));
    }

    #[test]
    fn comm_attachment_has_id_field() {
        let att = CommAttachment {
            id: Some("test-id".to_string()),
            filename: "test.pdf".to_string(),
            media_type: "application/pdf".to_string(),
            data: vec![1, 2, 3],
        };
        assert_eq!(att.id.as_deref(), Some("test-id"));
    }

    #[test]
    fn comm_attachment_id_defaults_to_none() {
        let att = CommAttachment {
            id: None,
            filename: "test.txt".to_string(),
            media_type: "text/plain".to_string(),
            data: Vec::new(),
        };
        assert!(att.id.is_none());
    }
}
