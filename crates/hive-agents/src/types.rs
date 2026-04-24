use std::sync::Arc;

use async_trait::async_trait;
use hive_classification::DataClass;
use hive_contracts::{LoopStrategy, PermissionRule, ReasoningEvent, ToolExecutionMode};
use hive_tools::ToolRegistry;
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::AgentError;

/// Factory for building persona-specific tool registries and skill catalogs.
///
/// When a child agent is spawned with a persona different from the session
/// persona, the supervisor calls this factory to obtain a tool registry
/// scoped to that persona's MCP servers, connectors, and skills — instead
/// of sharing the session-wide registry.
#[async_trait]
pub trait PersonaToolFactory: Send + Sync {
    /// Build a `ToolRegistry` and optional `SkillCatalog` scoped to the given
    /// persona. Implementations should create persona-specific MCP connections,
    /// connector tools, and skill catalogs.
    async fn build_tools_for_persona(
        &self,
        persona_id: &str,
        session_id: &str,
    ) -> Result<(Arc<ToolRegistry>, Option<Arc<hive_skills::SkillCatalog>>), AgentError>;
}

/// Deserialize a value that may be null, using the type's Default when null.
fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub friendly_name: String,
    pub description: String,
    pub role: AgentRole,
    pub model: Option<String>,
    /// Full preferred model pattern list from the persona config.
    /// Takes priority over the single `model` field for routing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_models: Option<Vec<String>>,
    /// When set, overrides the default loop strategy for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_strategy: Option<LoopStrategy>,
    /// When set, overrides the default tool execution mode for this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_execution_mode: Option<ToolExecutionMode>,
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub avatar: Option<String>,
    pub color: Option<String>,
    #[serde(default = "default_data_class")]
    pub data_class: DataClass,
    /// When true, the agent stays alive after completing a task and waits for
    /// more messages. When false (default), the agent terminates after its
    /// first task completes.
    #[serde(default)]
    pub keep_alive: bool,
    /// Maximum seconds the agent will wait for a new message while idle.
    /// Only applies when `keep_alive` is true. `None` means wait indefinitely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_timeout_secs: Option<u64>,
    /// Optional tool limits override. When `None`, uses system defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_limits: Option<hive_contracts::ToolLimitsConfig>,
    /// The persona this agent was created from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_id: Option<String>,
    /// When true, this agent's lifecycle is managed by the workflow engine.
    /// Session restore should skip re-sending tasks to these agents; the
    /// workflow recovery path handles re-spawning them.
    #[serde(default)]
    pub workflow_managed: bool,
    /// When true, side-effecting tool calls are intercepted and a synthetic
    /// success response is returned instead of executing the real tool.
    /// Read-only tools and built-in orchestration tools still pass through.
    /// Used by the workflow shadow/test-run system.
    #[serde(default)]
    pub shadow_mode: bool,
}

fn default_data_class() -> DataClass {
    DataClass::Public
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Researcher,
    Coder,
    Reviewer,
    Writer,
    Analyst,
    Custom(String),
}

impl Default for AgentRole {
    fn default() -> Self {
        Self::Custom("system/general".to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    Spawning,
    Active,
    Waiting,
    Paused,
    Blocked,
    Terminating,
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentMessage {
    Task { content: String, from: Option<String> },
    Result { content: String, artifacts: Vec<String> },
    Feedback { content: String, from: String },
    Broadcast { content: String, from: String },
    Directive { content: String },
    Control(ControlSignal),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlSignal {
    Pause,
    Resume,
    Kill,
}

/// Events emitted by the supervisor for external consumption.
/// The modality layer subscribes to these and renders them appropriately.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum SupervisorEvent {
    AgentSpawned {
        agent_id: String,
        spec: AgentSpec,
        parent_id: Option<String>,
    },
    AgentStatusChanged {
        agent_id: String,
        status: AgentStatus,
    },
    /// Emitted when an agent receives its first task, capturing the
    /// original task content for restart / persistence support.
    AgentTaskAssigned {
        agent_id: String,
        task: String,
    },
    MessageRouted {
        from: String,
        to: String,
        msg_type: String,
    },
    AgentOutput {
        agent_id: String,
        event: ReasoningEvent,
    },
    AgentCompleted {
        agent_id: String,
        result: String,
    },
    AllComplete {
        total_messages: u64,
    },
}

/// Summary of an agent's current state, returned by the supervisor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AgentSummary {
    pub agent_id: String,
    pub spec: AgentSpec,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
    pub tools: Vec<String>,
    /// The ID of the parent agent that spawned this agent, or None if spawned
    /// directly by the chat session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Unix epoch milliseconds when this agent was spawned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at_ms: Option<u64>,
    /// The top-level chat session this agent belongs to, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The final result produced by the agent, if it has completed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub final_result: Option<String>,
}

/// A pending approval request on an agent, returned by the supervisor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingAgentApproval {
    pub agent_id: String,
    pub agent_name: String,
    pub request_id: String,
    pub tool_id: String,
    pub input: String,
    pub reason: String,
}

/// A pending question from an agent, returned by the supervisor.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PendingAgentQuestion {
    pub agent_id: String,
    pub agent_name: String,
    pub request_id: String,
    pub text: String,
    pub choices: Vec<String>,
    pub allow_freeform: bool,
    /// When true, the user can select multiple choices at once.
    #[serde(default)]
    pub multi_select: bool,
    /// The assistant's accompanying message content (text produced alongside the tool call).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Topology definition for multi-agent flows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TopologyDef {
    pub agents: Vec<AgentSpec>,
    pub flows: Vec<FlowEdge>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FlowEdge {
    pub from: String,
    pub to: Vec<String>,
    pub flow_type: FlowType,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FlowType {
    Pipeline,
    FanOut,
    FanIn,
    Feedback,
}

// ── Bots ────────────────────────────────────────────────────────────────────

/// Execution mode for a bot.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum BotMode {
    /// Complete the launch prompt, then wait for new messages.
    #[default]
    IdleAfterTask,
    /// Run continuously with the system prompt as standing orders.
    Continuous,
    /// Complete the launch prompt and terminate.
    OneShot,
}

/// Configuration for a bot — lives independently of any session.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotConfig {
    #[serde(default)]
    pub id: String,
    pub friendly_name: String,
    /// Description visible to other agents for peer discovery.
    #[serde(default)]
    pub description: String,
    pub avatar: Option<String>,
    pub color: Option<String>,
    pub model: Option<String>,
    /// Full preferred model pattern list from the persona.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_models: Option<Vec<String>>,
    /// When set, overrides the default loop strategy for this bot's agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_strategy: Option<LoopStrategy>,
    /// When set, overrides the default tool execution mode for this bot's agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_execution_mode: Option<ToolExecutionMode>,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub system_prompt: String,
    /// Initial task or standing orders sent on activation.
    pub launch_prompt: String,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    #[serde(default = "default_data_class", deserialize_with = "deserialize_null_default")]
    pub data_class: DataClass,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub role: AgentRole,
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub mode: BotMode,
    /// Whether this bot should be running.
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub created_at: String,
    /// Maximum execution time in seconds for one-shot bots. Ignored when mode
    /// is not `OneShot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    /// Per-agent permission rules (tool approval policies).
    #[serde(default)]
    pub permission_rules: Vec<PermissionRule>,
    /// Optional tool limits override. When `None`, uses system defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_limits: Option<hive_contracts::ToolLimitsConfig>,
    /// The persona this bot was created from, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persona_id: Option<String>,
}

impl BotConfig {
    /// Convert to an `AgentSpec` suitable for the supervisor.
    pub fn to_agent_spec(&self) -> AgentSpec {
        AgentSpec {
            id: self.id.clone(),
            name: self.friendly_name.clone(),
            friendly_name: self.friendly_name.clone(),
            description: self.description.clone(),
            role: self.role.clone(),
            model: self.model.clone(),
            preferred_models: self.preferred_models.clone(),
            loop_strategy: self.loop_strategy.clone(),
            tool_execution_mode: self.tool_execution_mode,
            system_prompt: self.system_prompt.clone(),
            allowed_tools: self.allowed_tools.clone(),
            avatar: self.avatar.clone(),
            color: self.color.clone(),
            data_class: self.data_class,
            keep_alive: self.mode != BotMode::OneShot,
            idle_timeout_secs: if self.mode != BotMode::OneShot { self.timeout_secs } else { None },
            tool_limits: self.tool_limits.clone(),
            persona_id: self.persona_id.clone(),
            workflow_managed: false,
            shadow_mode: false,
        }
    }
}

/// Summary of a bot for the management UI.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BotSummary {
    pub config: BotConfig,
    pub status: AgentStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_model: Option<String>,
    pub tools: Vec<String>,
}

#[doc(hidden)]
pub type ServiceAgentMode = BotMode;

#[doc(hidden)]
pub type ServiceAgentConfig = BotConfig;

#[doc(hidden)]
pub type ServiceAgentSummary = BotSummary;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bot_config_from_tauri_payload() {
        // This is the exact JSON that the Tauri layer produces from the frontend form.
        let json = serde_json::json!({
            "friendly_name": "Test Agent",
            "description": "A test agent",
            "model": null,
            "launch_prompt": "Do stuff",
            "system_prompt": "",
            "mode": "idle_after_task",
            "allowed_tools": [],
            "data_class": "INTERNAL",
            "avatar": "🤖"
        });

        let result = serde_json::from_value::<BotConfig>(json);
        match &result {
            Ok(cfg) => {
                assert_eq!(cfg.friendly_name, "Test Agent");
                assert_eq!(cfg.launch_prompt, "Do stuff");
                assert_eq!(cfg.data_class, DataClass::Internal);
                assert_eq!(cfg.mode, BotMode::IdleAfterTask);
            }
            Err(e) => {
                panic!("Deserialization failed: {e}");
            }
        }
    }

    #[test]
    fn test_bot_config_continuous_mode() {
        let json = serde_json::json!({
            "friendly_name": "Watcher",
            "launch_prompt": "Monitor things",
            "mode": "continuous",
            "data_class": "PUBLIC"
        });

        let cfg: BotConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.mode, BotMode::Continuous);
        assert_eq!(cfg.data_class, DataClass::Public);
    }

    #[test]
    fn test_bot_config_minimal() {
        // Minimal payload — only required fields
        let json = serde_json::json!({
            "friendly_name": "Bot",
            "launch_prompt": "Hello"
        });

        let cfg: BotConfig = serde_json::from_value(json).unwrap();
        assert_eq!(cfg.friendly_name, "Bot");
    }

    #[test]
    fn test_bot_config_with_null_fields() {
        // This is the EXACT JSON that the Tauri BotConfigPayload
        // serializes when Option<String> fields are None.
        // #[serde(default)] only handles ABSENT fields, not null values.
        let json = serde_json::json!({
            "friendly_name": "Test Agent",
            "description": "A test",
            "avatar": "🤖",
            "color": null,
            "model": null,
            "system_prompt": "",
            "launch_prompt": "Do stuff",
            "allowed_tools": [],
            "data_class": "INTERNAL",
            "role": null,
            "mode": "idle_after_task"
        });

        let result = serde_json::from_value::<BotConfig>(json);
        match &result {
            Ok(cfg) => {
                assert_eq!(cfg.friendly_name, "Test Agent");
                assert_eq!(cfg.data_class, DataClass::Internal);
                assert_eq!(cfg.mode, BotMode::IdleAfterTask);
            }
            Err(e) => {
                panic!("Deserialization with null fields failed: {e}");
            }
        }
    }
}
