use hive_classification::ChannelClass;
use serde::{de, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const DEFAULT_CONFIG_FILE: &str = "config.yaml";
pub const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:9180";
pub const DEFAULT_OLLAMA_BASE_URL: &str = "http://localhost:11434/v1";

// ── HiveMindConfig ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
#[derive(Default)]
pub struct HiveMindConfig {
    pub daemon: DaemonConfig,
    pub api: ApiConfig,
    pub security: SecurityConfig,
    pub models: ModelsConfig,
    pub local_models: LocalModelsConfig,
    #[serde(default, skip_serializing)]
    pub hf_token: Option<String>,
    #[serde(default)]
    pub setup_completed: bool,
    #[serde(default)]
    pub skills: crate::skills::SkillsConfig,
    #[serde(default, skip_serializing)]
    pub personas: Vec<Persona>,
    #[serde(default)]
    pub compaction: ContextCompactionConfig,
    #[serde(default)]
    pub embedding: EmbeddingConfig,
    #[serde(default)]
    pub afk: AfkConfig,
    #[serde(default)]
    pub python: PythonConfig,
    #[serde(default)]
    pub node: NodeConfig,
    #[serde(default)]
    pub tool_limits: ToolLimitsConfig,
    #[serde(default)]
    pub web_search: WebSearchConfig,
    #[serde(default)]
    pub code_act: CodeActConfig,
}

impl HiveMindConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}", self.api.bind)
    }
}

// ── DaemonConfig ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DaemonConfig {
    pub log_level: String,
    pub event_bus_capacity: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self { log_level: "info".to_string(), event_bus_capacity: 512 }
    }
}

// ── ApiConfig ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ApiConfig {
    pub bind: String,
    pub http_enabled: bool,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self { bind: DEFAULT_BIND_ADDRESS.to_string(), http_enabled: true }
    }
}

// ── SecurityConfig ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
#[derive(Default)]
pub struct SecurityConfig {
    pub override_policy: OverridePolicyConfig,
    pub prompt_injection: PromptInjectionConfig,
    /// Default permission rules inherited by every new session.
    pub default_permissions: Vec<crate::permissions::PermissionRule>,
    /// Pre-execution command inspection for shell.execute / process.start.
    #[serde(default)]
    pub command_policy: CommandPolicyConfig,
    /// OS-level sandboxing for shell commands.
    #[serde(default)]
    pub sandbox: SandboxConfig,
}

// ── CommandPolicyConfig ─────────────────────────────────────────────

/// Pre-execution command inspection policy.
///
/// Shell commands issued by `shell.execute` and `process.start` are
/// matched against built-in and user-defined patterns before execution.
/// Each pattern belongs to a [`CommandRiskCategory`] whose default action
/// can be overridden here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct CommandPolicyConfig {
    /// Master switch. When `false` the configurable scanner is skipped,
    /// but the hardcoded hive-config meta-protection still runs.
    pub enabled: bool,
    /// Per-category action overrides.  Missing categories use their
    /// compiled-in defaults (see [`CommandRiskCategory::default_action`]).
    #[serde(default)]
    pub categories: BTreeMap<CommandRiskCategory, CommandPolicyAction>,
    /// Additional patterns defined by the user, merged with built-ins.
    #[serde(default)]
    pub custom_patterns: Vec<CustomCommandPattern>,
}

impl Default for CommandPolicyConfig {
    fn default() -> Self {
        Self { enabled: true, categories: BTreeMap::new(), custom_patterns: Vec::new() }
    }
}

/// Risk categories for shell commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRiskCategory {
    /// `rm -rf /`, `mkfs`, fork bombs, etc.
    DestructiveSystem,
    /// Reading credential stores piped to network commands.
    CredentialExfiltration,
    /// Reverse shells, `curl | sh`, data posting.
    NetworkExfiltration,
    /// `crontab`, shell rc files, SSH authorized_keys.
    Persistence,
    /// `base64 -d | sh`, `powershell -EncodedCommand`.
    ObfuscatedExecution,
}

impl CommandRiskCategory {
    /// Compiled-in default action when no config override is present.
    pub fn default_action(self) -> CommandPolicyAction {
        match self {
            Self::CredentialExfiltration => CommandPolicyAction::Block,
            _ => CommandPolicyAction::Warn,
        }
    }
}

/// What to do when a command matches a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicyAction {
    /// Skip the check entirely.
    Allow,
    /// Flag the command to the user with an explanation; let them override.
    Warn,
    /// Hard-refuse execution and return an error to the model.
    Block,
}

/// A user-defined command pattern.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomCommandPattern {
    /// Regex pattern to match against the normalised command string.
    pub pattern: String,
    /// Which risk category this pattern belongs to.
    pub category: CommandRiskCategory,
    /// Human-readable explanation shown to the user on match.
    pub description: String,
}

// ── SandboxConfig ─────────────────────────────────────────────────

/// OS-level sandboxing for shell commands.
///
/// When enabled, `shell.execute` and `process.start` commands are wrapped
/// in a platform-native sandbox that restricts filesystem and network access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SandboxConfig {
    /// Master switch. When `false` commands execute without any sandbox.
    pub enabled: bool,
    /// Additional paths to allow read-only access (beyond workspace + venv).
    #[serde(default)]
    pub extra_read_paths: Vec<String>,
    /// Additional paths to allow read-write access.
    #[serde(default)]
    pub extra_write_paths: Vec<String>,
    /// Allow network access in sandboxed commands.
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_read_paths: Vec::new(),
            extra_write_paths: Vec::new(),
            allow_network: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PolicyAction {
    Block,
    Prompt,
    Allow,
    RedactAndSend,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OverridePolicyConfig {
    pub internal: PolicyAction,
    pub confidential: PolicyAction,
    pub restricted: PolicyAction,
}

impl Default for OverridePolicyConfig {
    fn default() -> Self {
        Self {
            internal: PolicyAction::Prompt,
            confidential: PolicyAction::Prompt,
            restricted: PolicyAction::Block,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ScannerAction {
    Block,
    Prompt,
    Flag,
    Allow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PromptInjectionConfig {
    pub enabled: bool,
    pub action_on_detection: ScannerAction,
    pub confidence_threshold: f32,
    pub cache_ttl_secs: u64,
    /// When true, use an LLM model for scanning instead of heuristics.
    /// Off by default — adds latency and token cost per scanned payload.
    #[serde(default)]
    pub model_scanning_enabled: bool,
    /// Ordered list of models to use for model-based scanning.
    /// Tried in order; first available wins.
    #[serde(default)]
    pub scanner_models: Vec<ScannerModelEntry>,
    /// Per-source scan toggles.
    #[serde(default)]
    pub scan_sources: ScanSourceConfig,
    /// Maximum tokens per scan payload. Larger payloads are truncated.
    #[serde(default = "default_max_payload_tokens")]
    pub max_payload_tokens: usize,
    /// When true, combine small tool results into a single scan call.
    #[serde(default = "default_true")]
    pub batch_small_payloads: bool,
}

fn default_max_payload_tokens() -> usize {
    4096
}
fn default_true() -> bool {
    true
}

/// Per-source scan toggles. Each flag controls whether that data source
/// is scanned for prompt injection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ScanSourceConfig {
    /// Scan file contents when read by tools.
    pub workspace_files: bool,
    /// Scan clipboard content when pasted into the workspace.
    pub clipboard: bool,
    /// Scan inbound chat messages before they reach the agent.
    pub messaging_inbound: bool,
    /// Scan HTTP responses fetched by tools.
    pub web_content: bool,
    /// Scan responses from MCP servers (tools and resources).
    pub mcp_responses: bool,
    /// Per-tool overrides. Key is tool name, value is whether to scan.
    /// An absent key means "use the category default".
    #[serde(default)]
    pub tool_overrides: std::collections::HashMap<String, bool>,
}

impl Default for ScanSourceConfig {
    fn default() -> Self {
        Self {
            workspace_files: true,
            clipboard: true,
            messaging_inbound: true,
            web_content: true,
            mcp_responses: true,
            tool_overrides: std::collections::HashMap::new(),
        }
    }
}

/// A provider+model pair used in priority-ordered scanner model lists.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScannerModelEntry {
    pub provider: String,
    pub model: String,
}

impl Default for PromptInjectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            action_on_detection: ScannerAction::Prompt,
            confidence_threshold: 0.7,
            cache_ttl_secs: 3_600,
            model_scanning_enabled: false,
            scanner_models: Vec::new(),
            scan_sources: ScanSourceConfig::default(),
            max_payload_tokens: default_max_payload_tokens(),
            batch_small_payloads: true,
        }
    }
}

// ── ModelsConfig ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ModelsConfig {
    pub providers: Vec<ModelProviderConfig>,
    /// Timeout in seconds for synchronous (blocking) LLM completion calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout_secs: Option<u64>,
    /// Timeout in seconds for asynchronous (streaming) LLM completion calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_timeout_secs: Option<u64>,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            providers: default_provider_registry(),
            request_timeout_secs: None,
            stream_timeout_secs: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKindConfig {
    #[serde(alias = "azure-open-ai")]
    OpenAiCompatible,
    Anthropic,
    #[serde(rename = "microsoft-foundry", alias = "azure-foundry")]
    MicrosoftFoundry,
    #[serde(rename = "github-copilot")]
    GitHubCopilot,
    OllamaLocal,
    LocalModels,
    Mock,
}

impl ProviderKindConfig {
    pub fn default_base_url(self) -> Option<&'static str> {
        match self {
            Self::OllamaLocal => Some(DEFAULT_OLLAMA_BASE_URL),
            _ => None,
        }
    }

    pub fn requires_base_url(self) -> bool {
        !matches!(self, Self::Mock | Self::OllamaLocal | Self::LocalModels)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityConfig {
    Chat,
    Code,
    Vision,
    Embedding,
    ToolUse,
}

// ── ProviderAuthConfig ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ProviderAuthConfig {
    #[default]
    None,
    Env(String),
    GitHubOAuth,
    ApiKey,
}

impl ProviderAuthConfig {
    pub fn parse_spec(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err("provider auth must not be empty".to_string());
        }

        match trimmed {
            "none" => Ok(Self::None),
            "github-oauth" => Ok(Self::GitHubOAuth),
            "api-key" => Ok(Self::ApiKey),
            _ => {
                let Some(env_name) = trimmed.strip_prefix("env:") else {
                    return Err(format!(
                        "unsupported provider auth `{trimmed}` (expected `none`, `github-oauth`, `api-key`, or `env:VAR`)"
                    ));
                };

                if env_name.trim().is_empty() {
                    return Err("provider auth env spec must include an environment variable name"
                        .to_string());
                }

                if !env_name.trim().chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    return Err(format!(
                        "invalid environment variable name '{}': only ASCII alphanumeric and underscore allowed",
                        env_name.trim()
                    ));
                }

                Ok(Self::Env(env_name.trim().to_string()))
            }
        }
    }

    pub fn as_spec(&self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::Env(env_name) => format!("env:{env_name}"),
            Self::GitHubOAuth => "github-oauth".to_string(),
            Self::ApiKey => "api-key".to_string(),
        }
    }
}

impl Serialize for ProviderAuthConfig {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.as_spec())
    }
}

impl<'de> Deserialize<'de> for ProviderAuthConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse_spec(&raw).map_err(de::Error::custom)
    }
}

// ── ModelProviderConfig ─────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelProviderConfig {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: ProviderKindConfig,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub auth: ProviderAuthConfig,
    pub models: Vec<String>,
    /// Legacy provider-level capabilities. Kept for backward compat on
    /// deserialization — new configs store capabilities per-model in
    /// `model_capabilities` instead. Skipped on serialization.
    #[serde(default, skip_serializing)]
    pub capabilities: BTreeSet<CapabilityConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_capabilities: BTreeMap<String, BTreeSet<CapabilityConfig>>,
    pub channel_class: ChannelClass,
    #[serde(default = "default_provider_priority")]
    pub priority: i32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub options: ProviderOptionsConfig,
}

impl ModelProviderConfig {
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    pub fn resolved_base_url(&self) -> Option<String> {
        self.base_url
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .or_else(|| self.kind.default_base_url().map(str::to_string))
    }

    pub fn validate_base_url(&self) -> Result<(), String> {
        if self.kind.requires_base_url() && self.resolved_base_url().is_none() {
            return Err(format!(
                "provider `{}` of kind {:?} requires a non-empty `base_url`",
                self.display_name(),
                self.kind
            ));
        }
        Ok(())
    }

    /// Migrate legacy provider-level `capabilities` into `model_capabilities`.
    ///
    /// For each model that does not already have an explicit entry in
    /// `model_capabilities`, copy the provider-level capabilities as its
    /// default. Then clear the provider-level field.
    pub fn migrate_capabilities(&mut self) {
        if self.capabilities.is_empty() {
            return;
        }
        for model in &self.models {
            self.model_capabilities
                .entry(model.clone())
                .or_insert_with(|| self.capabilities.clone());
        }
        self.capabilities.clear();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct ProviderOptionsConfig {
    pub route: Option<String>,
    pub allow_model_discovery: bool,
    pub default_api_version: Option<String>,
    pub response_prefix: Option<String>,
    pub headers: BTreeMap<String, String>,
}

// ── Local model inference configuration ─────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum InferenceRuntimeKind {
    Candle,
    Onnx,
    LlamaCpp,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ModelTask {
    Chat,
    TextGeneration,
    Embedding,
    AudioTranscription,
    AudioGeneration,
    ImageGeneration,
    VideoGeneration,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LocalModelsConfig {
    pub enabled: bool,
    pub storage_path: Option<PathBuf>,
    pub max_loaded_models: usize,
    pub max_download_concurrent: usize,
    pub auto_evict: bool,
    /// When true, each inference runtime runs in an isolated child process.
    /// Protects the daemon from crashes in C++ FFI backends (llama.cpp, ONNX).
    pub isolate_runtimes: bool,
}

impl Default for LocalModelsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_path: None,
            max_loaded_models: 2,
            max_download_concurrent: 2,
            auto_evict: true,
            isolate_runtimes: true,
        }
    }
}

impl LocalModelsConfig {
    pub fn resolved_storage_path(&self) -> PathBuf {
        self.storage_path.clone().unwrap_or_else(|| PathBuf::from(".hivemind").join("models"))
    }
}

// ── Embedding configuration ─────────────────────────────────────────

/// Definition of an embedding model that can be used for vector search.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingModelDef {
    /// Unique identifier for this model (e.g. "bge-small-en-v1.5").
    pub model_id: String,
    /// HuggingFace repo (e.g. "BAAI/bge-small-en-v1.5").
    pub hub_repo: String,
    /// Filename within the repo (e.g. "onnx/model.onnx").
    pub filename: String,
    /// Output embedding dimension (e.g. 384).
    pub dimensions: usize,
    /// Inference runtime to use.
    #[serde(default = "default_embedding_runtime")]
    pub runtime: InferenceRuntimeKind,
}

fn default_embedding_runtime() -> InferenceRuntimeKind {
    InferenceRuntimeKind::Onnx
}

/// A rule mapping a file glob pattern to an embedding model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingRule {
    /// Glob pattern matched against relative file paths (e.g. "*.docx").
    pub glob: String,
    /// The `model_id` of the embedding model to use for matching files.
    pub model_id: String,
}

/// Top-level embedding configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct EmbeddingConfig {
    /// Available embedding models.
    pub models: Vec<EmbeddingModelDef>,
    /// Rules mapping file patterns to models.  Evaluated in order; first
    /// match wins.
    pub rules: Vec<EmbeddingRule>,
    /// Fallback model_id when no rule matches.
    pub default_model: String,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            models: vec![EmbeddingModelDef {
                model_id: "bge-small-en-v1.5".to_string(),
                hub_repo: "BAAI/bge-small-en-v1.5".to_string(),
                filename: "onnx/model.onnx".to_string(),
                dimensions: 384,
                runtime: InferenceRuntimeKind::Onnx,
            }],
            rules: Vec::new(),
            default_model: "bge-small-en-v1.5".to_string(),
        }
    }
}

// ── MCP configuration ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransportConfig {
    Stdio,
    Sse,
    StreamableHttp,
}

/// How an HTTP header value is stored — either as plain text or as a
/// reference to a secret in the OS keystore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "value", rename_all = "kebab-case")]
pub enum McpHeaderValue {
    Plain(String),
    SecretRef(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpServerConfig {
    pub id: String,
    pub transport: McpTransportConfig,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: BTreeMap<String, String>,
    /// HTTP headers to include on SSE/StreamableHTTP connections.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, McpHeaderValue>,
    pub channel_class: ChannelClass,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_enabled")]
    pub auto_connect: bool,
    /// When true, incoming MCP notifications trigger new agent turns automatically.
    #[serde(default = "default_enabled")]
    pub reactive: bool,
    /// Whether to automatically reconnect this server if it disconnects unexpectedly.
    /// Defaults to true.
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,
    /// Optional OS-level sandbox for stdio-transport MCP server subprocesses.
    #[serde(default)]
    pub sandbox: Option<McpSandboxConfig>,
}

/// Sandbox configuration for an individual MCP server subprocess.
///
/// Only applies to `Stdio` transport — SSE/HTTP servers have no local subprocess.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct McpSandboxConfig {
    /// Master switch. When `false` the server runs unsandboxed.
    pub enabled: bool,
    /// Allow read access to the session workspace directory.
    pub read_workspace: bool,
    /// Allow write access to the session workspace directory.
    pub write_workspace: bool,
    /// Allow network access.
    pub allow_network: bool,
    /// Additional paths to allow read-only access.
    #[serde(default)]
    pub extra_read_paths: Vec<String>,
    /// Additional paths to allow read-write access.
    #[serde(default)]
    pub extra_write_paths: Vec<String>,
}

impl Default for McpSandboxConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            read_workspace: true,
            write_workspace: false,
            allow_network: true,
            extra_read_paths: Vec::new(),
            extra_write_paths: Vec::new(),
        }
    }
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            transport: McpTransportConfig::Stdio,
            command: None,
            args: Vec::new(),
            url: None,
            env: BTreeMap::new(),
            headers: BTreeMap::new(),
            channel_class: ChannelClass::Internal,
            enabled: default_enabled(),
            auto_connect: default_enabled(),
            reactive: default_enabled(),
            auto_reconnect: default_true(),
            sandbox: None,
        }
    }
}

impl McpServerConfig {
    /// Content-addressed cache key based on the server's identity
    /// (transport, command, args, url).  Two personas with identical
    /// server configs will share the same catalog entry.
    pub fn cache_key(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(format!("{:?}", self.transport).as_bytes());
        if let Some(cmd) = &self.command {
            hasher.update(cmd.as_bytes());
        }
        for arg in &self.args {
            hasher.update(arg.as_bytes());
        }
        if let Some(url) = &self.url {
            hasher.update(url.as_bytes());
        }
        // Include env vars (BTreeMap is sorted, so order is deterministic).
        for (k, v) in &self.env {
            hasher.update(k.as_bytes());
            hasher.update(b"=");
            hasher.update(v.as_bytes());
        }
        // Include header names and values. For SecretRef, hash the ref key
        // (not the resolved secret) so the cache_key is config-stable.
        for (k, v) in &self.headers {
            hasher.update(k.as_bytes());
            match v {
                McpHeaderValue::Plain(s) => {
                    hasher.update(b":plain:");
                    hasher.update(s.as_bytes());
                }
                McpHeaderValue::SecretRef(key) => {
                    hasher.update(b":secret-ref:");
                    hasher.update(key.as_bytes());
                }
            }
        }
        format!("{:x}", hasher.finalize())
    }
}

// ── HiveMindPaths ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiveMindPaths {
    pub hivemind_home: PathBuf,
    pub config_path: PathBuf,
    pub personas_dir: PathBuf,
    pub run_dir: PathBuf,
    pub audit_log_path: PathBuf,
    pub knowledge_graph_path: PathBuf,
    pub risk_ledger_path: PathBuf,
    pub local_models_db_path: PathBuf,
    pub pid_file_path: PathBuf,
}

// ── Helper functions ────────────────────────────────────────────────

pub fn default_provider_capabilities() -> BTreeSet<CapabilityConfig> {
    [CapabilityConfig::Chat].into_iter().collect()
}

pub fn default_provider_priority() -> i32 {
    100
}

pub fn default_enabled() -> bool {
    true
}

pub fn default_provider_registry() -> Vec<ModelProviderConfig> {
    vec![
        // Local-models provider: auto-discovers downloaded models from
        // the local registry and serves them via the local inference runtime.
        ModelProviderConfig {
            id: "local".to_string(),
            name: Some("Local Models".to_string()),
            kind: ProviderKindConfig::LocalModels,
            base_url: None,
            auth: ProviderAuthConfig::None,
            models: vec![], // empty = auto-discover from registry
            capabilities: BTreeSet::new(),
            model_capabilities: BTreeMap::new(), // auto-populated at runtime
            channel_class: ChannelClass::LocalOnly,
            priority: 50,
            enabled: true,
            options: ProviderOptionsConfig::default(),
        },
    ]
}

// ── Agent Personas ───────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum LoopStrategy {
    #[default]
    React,
    Sequential,
    PlanThenExecute,
    CodeAct,
}

/// How batched tool calls from a single LLM response are executed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ToolExecutionMode {
    /// Execute sequentially; stop at the first failure and return partial
    /// results + the error to the LLM.
    #[default]
    SequentialPartial,
    /// Execute sequentially; continue past failures so the LLM sees all
    /// results (successes and errors).
    SequentialFull,
    /// Execute all tool calls concurrently and return all results.
    Parallel,
}

/// Backward-compatible deserializer: accepts a single string, a list of
/// strings, or null/missing.  A single string is promoted to a one-element
/// list so that existing configs with `preferred_model: "openai:gpt-5"` keep
/// working after the rename to `preferred_models`.
fn deserialize_preferred_models<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<Vec<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Visitor;

    struct StringOrVec;

    impl<'de> Visitor<'de> for StringOrVec {
        type Value = Option<Vec<String>>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("null, a string, or a list of strings")
        }

        fn visit_none<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_str<E>(self, value: &str) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(vec![value.to_owned()]))
            }
        }

        fn visit_string<E>(self, value: String) -> std::result::Result<Self::Value, E>
        where
            E: de::Error,
        {
            if value.is_empty() {
                Ok(None)
            } else {
                Ok(Some(vec![value]))
            }
        }

        fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
        where
            A: de::SeqAccess<'de>,
        {
            let mut items = Vec::new();
            while let Some(item) = seq.next_element::<String>()? {
                if !item.is_empty() {
                    items.push(item);
                }
            }
            if items.is_empty() {
                Ok(None)
            } else {
                Ok(Some(items))
            }
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

/// Strategy used to build a workspace context map that is appended to the
/// system prompt before each LLM invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum ContextMapStrategy {
    /// Lightweight, general-purpose context (default).
    #[default]
    General,
    /// Code-oriented context optimised for software-engineering tasks.
    Code,
    /// LLM-powered semantic architecture map.  Uses a secondary model to
    /// analyse workspace contents in a multi-pass pipeline and produce a
    /// rich, cached architectural overview.
    Advanced,
}

/// A reusable, parameterized prompt template attached to a persona.
///
/// The `template` field is a Handlebars template string.  Parameters are
/// described by an optional JSON Schema (`input_schema`) which follows the
/// same conventions as workflow manual-trigger `input_schema` – including
/// support for `x-ui` widget hints.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PromptTemplate {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Handlebars template string.
    pub template: String,
    /// JSON Schema defining the template parameters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Persona {
    /// Slash-separated path of arbitrary depth that serves as the unique
    /// identifier **and** determines on-disk location.
    /// Examples: `"system/general"`, `"user/my-agent"`, `"user/team/ops/monitor"`.
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default)]
    pub loop_strategy: LoopStrategy,
    #[serde(default)]
    pub tool_execution_mode: ToolExecutionMode,
    #[serde(
        default,
        alias = "preferred_model",
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_preferred_models"
    )]
    pub preferred_models: Option<Vec<String>>,
    #[serde(default)]
    pub allowed_tools: Vec<String>,
    /// MCP servers owned by this persona.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(default)]
    pub context_map_strategy: ContextMapStrategy,
    /// Preferred models for auxiliary / secondary LLM tasks such as context
    /// map generation and compaction.  Uses the same glob-pattern syntax as
    /// `preferred_models` (e.g. `["gpt-4.1-mini", "claude-haiku-*"]`).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_preferred_models"
    )]
    pub secondary_models: Option<Vec<String>>,
    /// When `true` the persona is hidden from normal listings but remains
    /// resolvable so that existing workflows referencing it keep working.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub archived: bool,
    /// `true` for factory-shipped personas that were seeded from bundled YAML
    /// embedded in the binary.  Bundled personas cannot be deleted, only
    /// archived (hidden).  Users can edit them and later reset to defaults.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bundled: bool,
    /// Reusable prompt templates that users can invoke from chat, the Agent
    /// Stage, Flight Deck, or the API.  Each template is a Handlebars string
    /// with an optional JSON Schema describing its parameters.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompts: Vec<PromptTemplate>,
}

impl Persona {
    /// Create the default "General Agent" persona.
    pub fn default_persona() -> Self {
        Self {
            id: "system/general".to_string(),
            name: "General Agent".to_string(),
            description: "Default agent with access to all tools and capabilities.".to_string(),
            system_prompt: String::new(),
            loop_strategy: LoopStrategy::React,
            tool_execution_mode: ToolExecutionMode::default(),
            preferred_models: None,
            allowed_tools: vec!["*".to_string()],
            mcp_servers: Vec::new(),
            avatar: Some("🤖".to_string()),
            color: Some("#89b4fa".to_string()),
            context_map_strategy: ContextMapStrategy::default(),
            secondary_models: None,
            archived: false,
            bundled: true,
            prompts: Vec::new(),
        }
    }

    /// Returns the root namespace segment (e.g. `"system"` for `"system/general"`).
    pub fn namespace_root(&self) -> &str {
        self.id.split('/').next().unwrap_or(&self.id)
    }

    /// Returns everything except the last segment (e.g. `"user/team"` for
    /// `"user/team/my-agent"`). Returns `""` when the ID has only one segment.
    pub fn parent_namespace(&self) -> &str {
        match self.id.rfind('/') {
            Some(pos) => &self.id[..pos],
            None => "",
        }
    }

    /// `true` when the persona lives under the `system/` namespace.
    pub fn is_system(&self) -> bool {
        self.namespace_root() == "system"
    }

    /// `true` when the persona lives under the `user/` namespace.
    pub fn is_user(&self) -> bool {
        self.namespace_root() == "user"
    }

    /// Validate that a persona ID is well-formed: at least two slash-separated
    /// segments where each segment contains only `[a-zA-Z0-9_-]`.
    pub fn validate_id(id: &str) -> Result<(), String> {
        validate_namespaced_id(id, "Persona ID")
    }
}

/// Validate a slash-separated namespaced identifier.
///
/// Rules:
/// - At least two segments separated by `/` (e.g. `"user/my-thing"`)
/// - Each segment must be non-empty and contain only `[a-zA-Z0-9_-]`
///
/// `label` is used in error messages (e.g. `"Persona ID"`, `"Workflow name"`).
pub fn validate_namespaced_id(id: &str, label: &str) -> Result<(), String> {
    let segments: Vec<&str> = id.split('/').collect();
    if segments.len() < 2 {
        return Err(format!("{label} must have at least two segments (e.g. 'user/my-thing')"));
    }
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            return Err(format!("{label} segment {i} is empty"));
        }
        if !seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err(format!(
                "{label} segment '{seg}' contains invalid characters (allowed: a-z, A-Z, 0-9, -, _)"
            ));
        }
    }
    Ok(())
}

// ── Context Compaction ──────────────────────────────────────────────

/// Strategy for context compaction when the conversation history approaches
/// the model's context window limit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[derive(Default)]
pub enum CompactionStrategy {
    /// Full pipeline: extract KG nodes + prose summary (default).
    ExtractAndSummarize,
    /// Prose summary only, no KG extraction.
    #[default]
    SummarizeOnly,
    /// Never auto-compact; user triggers via `/compact` command.
    Manual,
}

/// Configuration for automatic context compaction (SPEC.md §9.12).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextCompactionConfig {
    /// Which compaction strategy to use.
    #[serde(default)]
    pub strategy: CompactionStrategy,

    /// Fraction of the model's context window at which compaction triggers
    /// (e.g. 0.75 = compact at 75% usage).
    #[serde(default = "default_trigger_threshold")]
    pub trigger_threshold: f32,

    /// Always keep the last N turns in raw form (not compacted).
    #[serde(default = "default_keep_recent_turns")]
    pub keep_recent_turns: usize,

    /// Target size in tokens for each compaction summary.
    #[serde(default = "default_summary_max_tokens")]
    pub summary_max_tokens: u32,

    /// Model role used for extraction/summarization (cheap/fast).
    /// `None` means use the same model as the conversation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extraction_model: Option<String>,

    /// When summaries exceed this count, compact oldest summaries into an
    /// epoch summary (recursive compaction).
    #[serde(default = "default_max_summaries")]
    pub max_summaries_in_context: usize,
}

fn default_trigger_threshold() -> f32 {
    0.75
}
fn default_keep_recent_turns() -> usize {
    10
}
fn default_summary_max_tokens() -> u32 {
    800
}
fn default_max_summaries() -> usize {
    5
}

impl Default for ContextCompactionConfig {
    fn default() -> Self {
        Self {
            strategy: CompactionStrategy::default(),
            trigger_threshold: default_trigger_threshold(),
            keep_recent_turns: default_keep_recent_turns(),
            summary_max_tokens: default_summary_max_tokens(),
            extraction_model: None,
            max_summaries_in_context: default_max_summaries(),
        }
    }
}

// ── AFK / Status ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum UserStatus {
    #[default]
    Active,
    Idle,
    Away,
    DoNotDisturb,
}

impl std::fmt::Display for UserStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Active => write!(f, "active"),
            Self::Idle => write!(f, "idle"),
            Self::Away => write!(f, "away"),
            Self::DoNotDisturb => write!(f, "do_not_disturb"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AfkConfig {
    /// Which statuses trigger forwarding of interactions to the channel.
    pub forward_on: Vec<UserStatus>,
    /// Connector channel ID to forward interactions to (e.g. "my-slack").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_channel_id: Option<String>,
    /// Recipient address for email connectors (e.g. "you@example.com").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forward_to_address: Option<String>,
    /// Forward tool approval gates when AFK.
    pub forward_approvals: bool,
    /// Forward user question gates when AFK.
    pub forward_questions: bool,
    /// Auto-transition to Idle after N seconds of no UI heartbeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_idle_after_secs: Option<u64>,
    /// Auto-transition to Away after N seconds of no UI heartbeat.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_away_after_secs: Option<u64>,
    /// Auto-approve tool requests if no response within N seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_approve_on_timeout_secs: Option<u64>,
    /// Grace period (seconds) after daemon start before auto-transitioning to
    /// Away when no desktop client has ever sent a heartbeat.  Set to `null`
    /// to disable.  Default: 300 (5 minutes).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_client_grace_period_secs: Option<u64>,
}

impl Default for AfkConfig {
    fn default() -> Self {
        Self {
            forward_on: vec![UserStatus::Away, UserStatus::DoNotDisturb],
            forward_channel_id: None,
            forward_to_address: None,
            forward_approvals: true,
            forward_questions: true,
            auto_idle_after_secs: None,
            auto_away_after_secs: None,
            auto_approve_on_timeout_secs: None,
            no_client_grace_period_secs: Some(300),
        }
    }
}

// ── PythonConfig ────────────────────────────────────────────────────

/// Configuration for the managed Python environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct PythonConfig {
    /// Whether the managed Python environment is enabled.
    pub enabled: bool,
    /// Python version to install (e.g. "3.12").
    pub python_version: String,
    /// Packages to pre-install in the managed environment.
    pub base_packages: Vec<String>,
    /// Whether to auto-detect and install workspace dependencies.
    pub auto_detect_workspace_deps: bool,
    /// Pinned uv version to download.
    pub uv_version: String,
}

impl Default for PythonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            python_version: "3.12".to_string(),
            base_packages: vec![
                "requests".to_string(),
                "beautifulsoup4".to_string(),
                "pandas".to_string(),
                "numpy".to_string(),
                "pyyaml".to_string(),
                "python-dateutil".to_string(),
                "Pillow".to_string(),
                "matplotlib".to_string(),
                "jinja2".to_string(),
            ],
            auto_detect_workspace_deps: true,
            uv_version: "0.6.14".to_string(),
        }
    }
}

// ── Tool Limits ─────────────────────────────────────────────────────

/// Configuration for the managed Node.js environment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NodeConfig {
    /// Whether the managed Node.js environment is enabled.
    pub enabled: bool,
    /// Node.js version to install (e.g. "22.16.0" — current LTS).
    pub node_version: String,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self { enabled: true, node_version: "22.16.0".to_string() }
    }
}

// ── Tool Limits (continued) ─────────────────────────────────────────

/// Configuration for adaptive tool-call limits and stall detection.
///
/// Instead of a flat tool-call cap, the agent loop uses a soft limit that
/// auto-extends when forward progress is detected, up to a hard ceiling.
/// A stall detector watches for repeated identical tool calls and stops
/// the agent early if it appears stuck.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolLimitsConfig {
    /// Tool calls allowed before the first forward-progress check.
    #[serde(default = "default_tool_soft_limit")]
    pub soft_limit: usize,

    /// Absolute maximum tool calls — never exceeded, even with extensions.
    #[serde(default = "default_tool_hard_ceiling")]
    pub hard_ceiling: usize,

    /// Additional calls granted per extension when forward progress is detected.
    #[serde(default = "default_tool_extension_chunk")]
    pub extension_chunk: usize,

    /// Sliding window size for stall detection (number of recent calls tracked).
    #[serde(default = "default_stall_window")]
    pub stall_window: usize,

    /// Number of consecutive identical `(tool_name, arguments)` calls
    /// that triggers a stall warning.
    #[serde(default = "default_stall_threshold")]
    pub stall_threshold: usize,
}

fn default_tool_soft_limit() -> usize {
    25
}
fn default_tool_hard_ceiling() -> usize {
    200
}
fn default_tool_extension_chunk() -> usize {
    25
}
fn default_stall_window() -> usize {
    20
}
fn default_stall_threshold() -> usize {
    10
}

impl Default for ToolLimitsConfig {
    fn default() -> Self {
        Self {
            soft_limit: default_tool_soft_limit(),
            hard_ceiling: default_tool_hard_ceiling(),
            extension_chunk: default_tool_extension_chunk(),
            stall_window: default_stall_window(),
            stall_threshold: default_stall_threshold(),
        }
    }
}

/// Configuration for the built-in web search tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WebSearchConfig {
    /// Which search provider to use: "brave", "tavily", or "none" to disable.
    #[serde(default = "default_web_search_provider")]
    pub provider: String,

    /// API key for the chosen provider.  Supports:
    ///   - a literal key string
    ///   - `env:VAR_NAME` to read from an environment variable
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
}

fn default_web_search_provider() -> String {
    "none".to_string()
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self { provider: default_web_search_provider(), api_key: None }
    }
}

impl WebSearchConfig {
    /// Resolve the actual API key value (handling `env:VAR` references).
    ///
    /// For `keyring:` prefixed keys, callers must resolve them via the
    /// OS secret store before passing to this method. This method handles
    /// `env:VAR` and literal values only.
    pub fn resolve_api_key(&self) -> Option<String> {
        self.api_key.as_ref().and_then(|key| {
            if let Some(var) = key.strip_prefix("env:") {
                std::env::var(var).ok()
            } else if key.starts_with("keyring:") {
                // Keyring references must be resolved at a higher layer
                // (e.g., hive-api or hive-chat) that has access to the
                // secret store. Return None here so the tool is disabled
                // until the reference is resolved.
                None
            } else {
                Some(key.clone())
            }
        })
    }
}

// ── CodeAct Sandbox Config ─────────────────────────────────────────

/// Configuration for the CodeAct sandbox (Python code execution).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeActConfig {
    /// Whether CodeAct code execution is enabled.
    pub enabled: bool,

    /// Execution timeout per code block in seconds.
    pub execution_timeout_secs: u64,

    /// Maximum output size in bytes before truncation.
    pub max_output_bytes: usize,

    /// Session idle timeout in seconds before the REPL is reaped.
    pub idle_timeout_secs: u64,

    /// Maximum number of concurrent executor sessions.
    pub max_sessions: usize,

    /// Whether to allow network access from executed code.
    pub allow_network: bool,
}

impl Default for CodeActConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            execution_timeout_secs: 30,
            max_output_bytes: 1_048_576, // 1 MB
            idle_timeout_secs: 600,      // 10 min
            max_sessions: 3,
            allow_network: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_kind_local_models_serializes_to_kebab_case() {
        let json = serde_json::to_string(&ProviderKindConfig::LocalModels).unwrap();
        assert_eq!(json, "\"local-models\"");
    }

    #[test]
    fn provider_kind_local_models_deserializes_from_kebab_case() {
        let kind: ProviderKindConfig = serde_json::from_str("\"local-models\"").unwrap();
        assert_eq!(kind, ProviderKindConfig::LocalModels);
    }

    #[test]
    fn provider_kind_round_trip_all_variants() {
        let variants = [
            (ProviderKindConfig::OpenAiCompatible, "\"open-ai-compatible\""),
            (ProviderKindConfig::Anthropic, "\"anthropic\""),
            (ProviderKindConfig::MicrosoftFoundry, "\"microsoft-foundry\""),
            (ProviderKindConfig::GitHubCopilot, "\"github-copilot\""),
            (ProviderKindConfig::OllamaLocal, "\"ollama-local\""),
            (ProviderKindConfig::LocalModels, "\"local-models\""),
            (ProviderKindConfig::Mock, "\"mock\""),
        ];
        for (variant, expected_json) in variants {
            let json = serde_json::to_string(&variant).unwrap();
            assert_eq!(json, expected_json, "serialize {variant:?}");
            let deserialized: ProviderKindConfig = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, variant, "deserialize {json}");
        }
    }

    #[test]
    fn azure_open_ai_alias_deserializes_to_open_ai_compatible() {
        let deserialized: ProviderKindConfig = serde_json::from_str("\"azure-open-ai\"").unwrap();
        assert_eq!(deserialized, ProviderKindConfig::OpenAiCompatible);
    }

    #[test]
    fn local_models_provider_config_round_trip() {
        let config = ModelProviderConfig {
            id: "my-local".to_string(),
            name: None,
            kind: ProviderKindConfig::LocalModels,
            base_url: None,
            auth: ProviderAuthConfig::None,
            models: vec!["phi-3-mini".to_string()],
            capabilities: [CapabilityConfig::Chat].into_iter().collect(),
            model_capabilities: BTreeMap::new(),
            channel_class: ChannelClass::LocalOnly,
            priority: 50,
            enabled: true,
            options: ProviderOptionsConfig::default(),
        };

        let json = serde_json::to_string_pretty(&config).unwrap();
        assert!(json.contains("\"local-models\""), "kind should serialize as local-models");

        let deserialized: ModelProviderConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "my-local");
        assert_eq!(deserialized.kind, ProviderKindConfig::LocalModels);
        assert_eq!(deserialized.models, vec!["phi-3-mini"]);
    }

    #[test]
    fn local_models_does_not_require_base_url() {
        assert!(!ProviderKindConfig::LocalModels.requires_base_url());
    }

    #[test]
    fn local_models_config_defaults() {
        let cfg = LocalModelsConfig::default();
        assert!(cfg.enabled);
        assert_eq!(cfg.max_loaded_models, 2);
        assert_eq!(cfg.max_download_concurrent, 2);
        assert!(cfg.auto_evict);
        assert!(cfg.storage_path.is_none());
    }

    #[test]
    fn local_models_config_resolved_storage_path_default() {
        let cfg = LocalModelsConfig::default();
        let resolved = cfg.resolved_storage_path();
        assert_eq!(resolved, PathBuf::from(".hivemind").join("models"));
    }

    #[test]
    fn local_models_config_resolved_storage_path_custom() {
        let cfg = LocalModelsConfig {
            storage_path: Some(PathBuf::from("/custom/path")),
            ..LocalModelsConfig::default()
        };
        assert_eq!(cfg.resolved_storage_path(), PathBuf::from("/custom/path"));
    }

    #[test]
    fn azure_foundry_backward_compatibility() {
        // Old name should still deserialize to MicrosoftFoundry
        let old: ProviderKindConfig = serde_json::from_str("\"azure-foundry\"").unwrap();
        assert_eq!(old, ProviderKindConfig::MicrosoftFoundry);

        // Serialization should use the new canonical name
        let json = serde_json::to_string(&ProviderKindConfig::MicrosoftFoundry).unwrap();
        assert_eq!(json, "\"microsoft-foundry\"");
    }

    #[test]
    fn persona_preferred_models_deserializes_single_string() {
        let json = r#"{"id":"p1","name":"Test","preferred_model":"gpt-5.2"}"#;
        let persona: Persona = serde_json::from_str(json).unwrap();
        assert_eq!(persona.preferred_models, Some(vec!["gpt-5.2".to_string()]));
    }

    #[test]
    fn persona_preferred_models_deserializes_list() {
        let json = r#"{"id":"p1","name":"Test","preferred_models":["gpt-5.*","claude-*"]}"#;
        let persona: Persona = serde_json::from_str(json).unwrap();
        assert_eq!(
            persona.preferred_models,
            Some(vec!["gpt-5.*".to_string(), "claude-*".to_string()])
        );
    }

    #[test]
    fn persona_preferred_models_deserializes_null() {
        let json = r#"{"id":"p1","name":"Test"}"#;
        let persona: Persona = serde_json::from_str(json).unwrap();
        assert_eq!(persona.preferred_models, None);
    }

    #[test]
    fn persona_preferred_models_serializes_as_list() {
        let persona = Persona {
            preferred_models: Some(vec!["gpt-5.*".to_string()]),
            ..Persona::default_persona()
        };
        let json = serde_json::to_string(&persona).unwrap();
        assert!(json.contains(r#""preferred_models":["gpt-5.*"]"#));
    }
}
