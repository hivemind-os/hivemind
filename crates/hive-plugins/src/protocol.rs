//! JSON-RPC protocol definitions for plugin ↔ host communication.
//!
//! Extends the standard MCP protocol with plugin-specific methods.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

// ─── JSON-RPC Types ─────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(method: &str, params: Option<Value>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            jsonrpc: "2.0".into(),
            id: Some(Value::Number(NEXT_ID.fetch_add(1, Ordering::Relaxed).into())),
            method: method.into(),
            params,
        }
    }

    pub fn notification(method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: None,
            method: method.into(),
            params,
        }
    }
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Value, code: i64, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// ─── Host → Plugin Methods ──────────────────────────────────────────────────

/// Methods the host can call on the plugin.
pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const CONFIG_SCHEMA: &str = "plugin/configSchema";
    pub const VALIDATE_CONFIG: &str = "plugin/validateConfig";
    pub const ACTIVATE: &str = "plugin/activate";
    pub const DEACTIVATE: &str = "plugin/deactivate";
    pub const START_LOOP: &str = "plugin/startLoop";
    pub const STOP_LOOP: &str = "plugin/stopLoop";
    pub const STATUS: &str = "plugin/status";
    pub const TOOLS_LIST: &str = "tools/list";
    pub const TOOLS_CALL: &str = "tools/call";
}

/// Methods the plugin can call on the host.
pub mod host_methods {
    pub const EMIT_MESSAGE: &str = "host/emitMessage";
    pub const EMIT_MESSAGES: &str = "host/emitMessages";
    pub const SECRET_GET: &str = "host/secretGet";
    pub const SECRET_SET: &str = "host/secretSet";
    pub const SECRET_DELETE: &str = "host/secretDelete";
    pub const SECRET_HAS: &str = "host/secretHas";
    pub const STORE_GET: &str = "host/storeGet";
    pub const STORE_SET: &str = "host/storeSet";
    pub const STORE_DELETE: &str = "host/storeDelete";
    pub const STORE_KEYS: &str = "host/storeKeys";
    pub const LOG: &str = "host/log";
    pub const NOTIFY: &str = "host/notify";
    pub const EMIT_EVENT: &str = "host/emitEvent";
    pub const UPDATE_STATUS: &str = "host/updateStatus";
    pub const SCHEDULE: &str = "host/schedule";
    pub const UNSCHEDULE: &str = "host/unschedule";
    pub const HTTP_FETCH: &str = "host/httpFetch";
    pub const FS_RESOLVE: &str = "host/fsResolve";
    pub const FS_READ: &str = "host/fsRead";
    pub const FS_WRITE: &str = "host/fsWrite";
    pub const FS_READ_DIR: &str = "host/fsReadDir";
    pub const FS_EXISTS: &str = "host/fsExists";
    pub const FS_MKDIR: &str = "host/fsMkdir";
    pub const FS_REMOVE: &str = "host/fsRemove";
    pub const LIST_CONNECTORS: &str = "host/listConnectors";
    pub const LIST_PERSONAS: &str = "host/listPersonas";
}

// ─── Protocol Message Types ─────────────────────────────────────────────────

/// Initialize request params.
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub plugin_id: String,
    pub config: Value,
    pub host_info: HostInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostInfo {
    pub version: String,
    pub platform: String,
    pub capabilities: Vec<String>,
}

/// Plugin status (from plugin/status or host/updateStatus).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginStatus {
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
}

/// Incoming message emitted by a plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IncomingMessage {
    pub source: String,
    pub channel: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender: Option<MessageSender>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub classification: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<MessageAttachment>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageSender {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAttachment {
    pub name: String,
    pub mime_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Tool definition from tools/list response.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDef {
    pub name: String,
    pub description: String,
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default)]
    pub annotations: Value,
}
