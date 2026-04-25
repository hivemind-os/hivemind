use hive_classification::DataClass;
use serde::{Deserialize, Serialize};

use crate::risk::{FileAuditStatus, PromptInjectionReview, ScanDecision, ScanSummary};

/// An image or file attached to a chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageAttachment {
    /// Unique attachment id (e.g. "att-0").
    pub id: String,
    /// Original filename, if available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    /// MIME type (e.g. "image/png").
    #[serde(alias = "mediaType")]
    pub media_type: String,
    /// Base64-encoded binary data.
    pub data: String,
}

/// The interaction modality for a chat session.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionModality {
    #[default]
    Linear,
    Spatial,
}

/// Semantic events emitted by the agent loop during reasoning.
/// These are modality-agnostic — the active modality decides how to render them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningEvent {
    StepStarted {
        step_id: String,
        description: String,
    },
    ModelCallStarted {
        model: String,
        prompt_preview: String,
        /// Count of tool results by tool name included in this model call.
        #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
        tool_result_counts: std::collections::HashMap<String, u32>,
        /// Estimated token count for the outgoing request.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        estimated_tokens: Option<u32>,
    },
    ModelCallCompleted {
        content: String,
        token_count: u32,
        #[serde(default)]
        model: String,
    },
    ToolCallStarted {
        tool_id: String,
        input: serde_json::Value,
    },
    ToolCallCompleted {
        tool_id: String,
        output: serde_json::Value,
        is_error: bool,
    },
    BranchEvaluated {
        condition: String,
        result: bool,
    },
    PathAbandoned {
        reason: String,
    },
    Synthesized {
        sources: Vec<String>,
        result: String,
    },
    Completed {
        result: String,
    },
    Failed {
        error: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
        /// Provider that produced the error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider_id: Option<String>,
        /// Model that produced the error.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
    },
    TokenDelta {
        token: String,
    },
    UserInteractionRequired {
        request_id: String,
        tool_id: String,
        input: String,
        reason: String,
    },
    /// Agent is asking the user a question via core.ask_user.
    QuestionAsked {
        request_id: String,
        agent_id: String,
        text: String,
        choices: Vec<String>,
        allow_freeform: bool,
        /// When true, the user can select multiple choices at once.
        #[serde(default)]
        multi_select: bool,
        /// The assistant's accompanying message content (text produced alongside the tool call).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A transient LLM error triggered a retry with backoff.
    ModelRetry {
        provider_id: String,
        model: String,
        attempt: u32,
        max_attempts: u32,
        error_kind: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        http_status: Option<u16>,
        backoff_ms: u64,
    },
    /// A side-effecting tool call was intercepted in shadow mode.
    /// The tool was NOT executed; a synthetic success was returned to the agent.
    ToolCallIntercepted {
        tool_id: String,
        input: serde_json::Value,
    },
    /// Partial tool-call argument snapshot during LLM streaming.
    ToolCallArgDelta {
        index: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        arguments_so_far: String,
    },
    /// CodeAct: a Python code block was executed.
    CodeExecution {
        code: String,
        output: String,
        is_error: bool,
    },
}

/// Defines how a modality stores conversation data and assembles context.
/// Each modality implementation handles ReasoningEvents differently.
#[async_trait::async_trait]
pub trait ConversationModality: Send + Sync {
    /// Store a new user message
    async fn append_user_message(
        &self,
        session_id: &str,
        content: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Handle a semantic reasoning event from the agent loop
    async fn handle_reasoning_event(
        &self,
        session_id: &str,
        event: &ReasoningEvent,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;

    /// Assemble context for the next model call
    async fn assemble_context(
        &self,
        session_id: &str,
        token_budget: usize,
    ) -> Result<Vec<ChatMessage>, Box<dyn std::error::Error + Send + Sync>>;

    /// Get the full session snapshot for frontend rendering
    async fn get_snapshot(
        &self,
        session_id: &str,
    ) -> Result<ChatSessionSnapshot, Box<dyn std::error::Error + Send + Sync>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatRunState {
    Idle,
    Running,
    Paused,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ChatMessageRole {
    #[default]
    User,
    Assistant,
    System,
    /// A message injected by another part of the system (workflow result,
    /// peer/child agent message, etc.). Included in LLM conversation history
    /// so the main agent has context about what happened.
    Notification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChatMessageStatus {
    Queued,
    Processing,
    Complete,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InterruptMode {
    Soft,
    Hard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: String,
    pub role: ChatMessageRole,
    pub status: ChatMessageStatus,
    pub content: String,
    #[serde(alias = "dataClass")]
    pub data_class: Option<DataClass>,
    #[serde(alias = "classificationReason")]
    pub classification_reason: Option<String>,
    #[serde(alias = "providerId")]
    pub provider_id: Option<String>,
    pub model: Option<String>,
    #[serde(alias = "scanSummary")]
    pub scan_summary: Option<ScanSummary>,
    pub intent: Option<String>,
    pub thinking: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    /// Links this message to an interaction gate (question or tool approval).
    /// When set, the frontend renders this as an interactive element.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_request_id: Option<String>,
    /// `"question"` or `"tool_approval"` — determines rendering style.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_kind: Option<String>,
    /// Structured metadata for the interaction (choices, allow_freeform, agent info, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_meta: Option<serde_json::Value>,
    /// Set when the interaction has been answered. Contains the answer text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_answer: Option<String>,
    #[serde(alias = "createdAtMs")]
    pub created_at_ms: u64,
    #[serde(alias = "updatedAtMs")]
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionSummary {
    pub id: String,
    pub title: String,
    pub modality: SessionModality,
    pub workspace_path: String,
    pub workspace_linked: bool,
    pub state: ChatRunState,
    pub queued_count: usize,
    pub updated_at_ms: u64,
    pub last_message_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSessionSnapshot {
    pub id: String,
    pub title: String,
    pub modality: SessionModality,
    pub workspace_path: String,
    pub workspace_linked: bool,
    pub state: ChatRunState,
    pub queued_count: usize,
    pub active_stage: Option<String>,
    pub active_intent: Option<String>,
    pub active_thinking: Option<String>,
    pub last_error: Option<String>,
    pub recalled_memories: Vec<ChatMemoryItem>,
    pub messages: Vec<ChatMessage>,
    pub permissions: crate::permissions::SessionPermissions,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    /// When set, this session is the backing session for a bot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot_id: Option<String>,
    /// The active persona for this session, used to determine which MCP
    /// servers are available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub children: Option<Vec<WorkspaceEntry>>,
    pub audit_status: Option<FileAuditStatus>,
    pub effective_classification: Option<DataClass>,
    pub has_classification_override: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceFileContent {
    pub path: String,
    pub content: String,
    pub is_binary: bool,
    pub mime_type: String,
    pub size: u64,
    /// When true the frontend should not allow editing this file.
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFileRequest {
    #[serde(default)]
    pub content: String,
    /// Optional base64-encoded binary content. If present, takes precedence over `content`.
    #[serde(default)]
    pub content_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMemoryItem {
    pub id: i64,
    pub node_type: String,
    pub name: String,
    pub data_class: DataClass,
    pub content: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub content: String,
    pub scan_decision: Option<ScanDecision>,
    #[serde(
        default,
        alias = "preferredModel",
        alias = "preferredModels",
        skip_serializing_if = "Option::is_none"
    )]
    pub preferred_models: Option<Vec<String>>,
    pub data_class_override: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Message role. Defaults to `User` when not specified.
    #[serde(default)]
    pub role: ChatMessageRole,
    /// Canvas position (x, y) for spatial sessions. When provided, spatial
    /// context assembly uses this position instead of the canvas centroid.
    #[serde(default)]
    pub canvas_position: Option<(f64, f64)>,
    /// Tools to exclude from this session (by tool id).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_tools: Option<Vec<String>>,
    /// Skills to exclude from this session (by skill name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub excluded_skills: Option<Vec<String>>,
    /// Image or file attachments to include with this message.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    /// When `true`, the message is queued without preempting the
    /// currently running agent turn. Defaults to `false` (preempt
    /// at the next checkpoint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip_preempt: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SendMessageResponse {
    Queued { session: ChatSessionSnapshot },
    ReviewRequired { review: PromptInjectionReview },
    Blocked { reason: String, summary: ScanSummary },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterruptRequest {
    pub mode: InterruptMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalRequest {
    pub request_id: String,
    pub approved: bool,
    /// When true, creates a session permission rule so this tool+scope
    /// is auto-approved (or auto-denied) for the rest of the session
    /// (all agents in the session).
    #[serde(default)]
    pub allow_session: bool,
    /// When true, creates a permission rule for the specific agent only.
    #[serde(default)]
    pub allow_agent: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolApprovalResponse {
    pub acknowledged: bool,
    /// The scope that was inferred and saved (if `allow_session` was true).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted_scope: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasoning_event_token_delta_serde_roundtrip() {
        let event = ReasoningEvent::TokenDelta { token: "Hello".to_string() };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"token_delta""#));
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::TokenDelta { token } => assert_eq!(token, "Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_event_tool_call_started_serde_roundtrip() {
        let event = ReasoningEvent::ToolCallStarted {
            tool_id: "mcp.server.tool".to_string(),
            input: serde_json::json!({"key": "value"}),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call_started""#));
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::ToolCallStarted { tool_id, input } => {
                assert_eq!(tool_id, "mcp.server.tool");
                assert_eq!(input["key"], "value");
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_event_tool_call_completed_serde_roundtrip() {
        let event = ReasoningEvent::ToolCallCompleted {
            tool_id: "test.tool".to_string(),
            output: serde_json::json!({"result": 42}),
            is_error: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::ToolCallCompleted { tool_id, output, is_error } => {
                assert_eq!(tool_id, "test.tool");
                assert_eq!(output["result"], 42);
                assert!(!is_error);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_event_model_call_started_serde_roundtrip() {
        let event = ReasoningEvent::ModelCallStarted {
            model: "gpt-4".to_string(),
            prompt_preview: "Hello".to_string(),
            tool_result_counts: std::collections::HashMap::new(),
            estimated_tokens: Some(100),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::ModelCallStarted { model, estimated_tokens, .. } => {
                assert_eq!(model, "gpt-4");
                assert_eq!(estimated_tokens, Some(100));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_event_failed_serde_roundtrip() {
        let event = ReasoningEvent::Failed {
            error: "timeout".to_string(),
            error_code: Some("timeout".to_string()),
            http_status: Some(504),
            provider_id: Some("openai".to_string()),
            model: Some("gpt-4".to_string()),
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::Failed { error, http_status, .. } => {
                assert_eq!(error, "timeout");
                assert_eq!(http_status, Some(504));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn reasoning_event_model_retry_serde_roundtrip() {
        let event = ReasoningEvent::ModelRetry {
            provider_id: "openai".to_string(),
            model: "gpt-4".to_string(),
            attempt: 1,
            max_attempts: 3,
            error_kind: "rate_limited".to_string(),
            http_status: Some(429),
            backoff_ms: 5000,
        };
        let json = serde_json::to_string(&event).unwrap();
        let parsed: ReasoningEvent = serde_json::from_str(&json).unwrap();
        match parsed {
            ReasoningEvent::ModelRetry { attempt, max_attempts, backoff_ms, .. } => {
                assert_eq!(attempt, 1);
                assert_eq!(max_attempts, 3);
                assert_eq!(backoff_ms, 5000);
            }
            _ => panic!("wrong variant"),
        }
    }
}
