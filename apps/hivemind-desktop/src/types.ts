export type DaemonStatus = {
  version: string;
  uptime_secs: number;
  pid: number;
  platform: string;
  bind: string;
};

export type AppContext = {
  daemon_url: string;
  config_path: string;
  knowledge_graph_path: string;
  risk_ledger_path: string;
};

export interface PromptTemplate {
  id: string;
  name: string;
  description?: string;
  template: string;
  input_schema?: Record<string, any>;
}

export interface McpHeaderValue {
  type: 'plain' | 'secret-ref';
  value: string;
}

export interface McpServerConfig {
  id: string;
  transport: TransportKind;
  command: string | null;
  args: string[];
  url: string | null;
  env: Record<string, string>;
  headers: Record<string, McpHeaderValue>;
  channel_class: string;
  enabled: boolean;
  auto_connect: boolean;
  reactive: boolean;
  auto_reconnect: boolean;
  sandbox?: {
    enabled: boolean;
    read_workspace: boolean;
    write_workspace: boolean;
    allow_network: boolean;
    extra_read_paths: string[];
    extra_write_paths: string[];
  } | null;
}

export interface Persona {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  loop_strategy: 'react' | 'sequential' | 'plan_then_execute';
  preferred_models?: string[] | null;
  secondary_models?: string[] | null;
  allowed_tools: string[];
  mcp_servers: McpServerConfig[];
  avatar?: string | null;
  color?: string | null;
  archived?: boolean;
  bundled?: boolean;
  prompts?: PromptTemplate[];
}

export interface AgentSpec {
  id: string;
  name: string;
  friendly_name: string;
  description: string;
  role: 'planner' | 'researcher' | 'coder' | 'reviewer' | 'writer' | 'analyst' | 'custom' | 'assistant';
  model?: string | null;
  system_prompt: string;
  allowed_tools: string[];
  avatar?: string | null;
  color?: string | null;
  persona_id?: string | null;
  data_class?: string;
  keep_alive?: boolean;
}

export type AgentStatus = 'spawning' | 'active' | 'waiting' | 'paused' | 'blocked' | 'done' | 'error';

export interface ModelUsage {
  input_tokens: number;
  output_tokens: number;
  calls: number;
}

export interface TokenUsage {
  input_tokens: number;
  output_tokens: number;
  model_calls: number;
  tool_calls: number;
  per_model: Record<string, ModelUsage>;
}

export interface TelemetrySnapshot {
  per_agent: [string, TokenUsage][];
  total: TokenUsage;
}

export type ChatRunState = 'idle' | 'running' | 'paused' | 'interrupted';
export type ChatMessageRole = 'user' | 'assistant' | 'system' | 'notification';
export type ChatMessageStatus = 'queued' | 'processing' | 'complete' | 'interrupted' | 'failed';
export type DataClass = 'PUBLIC' | 'INTERNAL' | 'CONFIDENTIAL' | 'RESTRICTED';

export type MessageAttachment = {
  id: string;
  filename?: string;
  media_type: string;
  data: string; // base64-encoded
};

export type ChatMessage = {
  id: string;
  role: ChatMessageRole;
  status: ChatMessageStatus;
  content: string;
  data_class: DataClass | null;
  classification_reason: string | null;
  provider_id: string | null;
  model: string | null;
  scan_summary:
    | {
        verdict: 'clean' | 'suspicious' | 'detected';
        confidence: number;
        action_taken: 'passed' | 'blocked' | 'redacted' | 'user_allowed' | 'user_blocked' | 'flagged';
      }
    | null;
  intent: string | null;
  thinking: string | null;
  attachments: MessageAttachment[];
  /** Links this message to an interaction gate (question or approval). */
  interaction_request_id?: string | null;
  /** "question" | "tool_approval" — determines how frontend renders it. */
  interaction_kind?: string | null;
  /** Structured interaction metadata (choices, allow_freeform, etc.) */
  interaction_meta?: {
    agent_id?: string;
    agent_name?: string;
    choices?: string[];
    allow_freeform?: boolean;
    multi_select?: boolean;
    message?: string | null;
    workflow_instance_id?: number | null;
    workflow_step_id?: string | null;
  } | null;
  /** When the interaction has been answered, contains the answer text. */
  interaction_answer?: string | null;
  created_at_ms: number;
  updated_at_ms: number;
};

export type SessionModality = 'linear' | 'spatial';

export type ChatSessionSummary = {
  id: string;
  title: string;
  modality: SessionModality;
  state: ChatRunState;
  queued_count: number;
  updated_at_ms: number;
  last_message_preview: string | null;
  workspace_path: string;
  workspace_linked: boolean;
  bot_id?: string | null;
};

export type ChatSessionSnapshot = {
  id: string;
  title: string;
  modality: SessionModality;
  state: ChatRunState;
  queued_count: number;
  active_stage: string | null;
  active_intent: string | null;
  active_thinking: string | null;
  last_error: string | null;
  recalled_memories: ChatMemoryItem[];
  messages: ChatMessage[];
  workspace_path: string;
  workspace_linked: boolean;
  created_at_ms: number;
  updated_at_ms: number;
  /** When set, this session is the backing session for a bot. */
  bot_id?: string | null;
  /** The active persona for this session. */
  persona_id?: string | null;
};

export type ChatMemoryItem = {
  id: number;
  node_type: string;
  name: string;
  data_class: DataClass;
  content: string | null;
};

export type ScanDecision = 'allow' | 'block' | 'redact';

export type PromptInjectionReview = {
  source: string;
  verdict: 'clean' | 'suspicious' | 'detected';
  confidence: number;
  threat_type: string | null;
  flagged_spans: Array<{ start: number; end: number; reason: string }>;
  recommendation: 'pass' | 'redact' | 'block';
  preview: string;
  proposed_redaction: string | null;
};

export type SendMessageResponse =
  | { kind: 'queued'; session: ChatSessionSnapshot }
  | { kind: 'review-required'; review: PromptInjectionReview }
  | {
      kind: 'blocked';
      reason: string;
      summary: {
        verdict: 'clean' | 'suspicious' | 'detected';
        confidence: number;
        action_taken: 'passed' | 'blocked' | 'redacted' | 'user_allowed' | 'user_blocked' | 'flagged';
      };
    };

export type RiskScanRecord = {
  id: string;
  scan_type: string;
  payload_hash: string;
  payload_preview: string;
  source: string;
  source_session: string | null;
  verdict: 'clean' | 'suspicious' | 'detected';
  confidence: number;
  threat_type: string | null;
  flagged_spans: Array<{ start: number; end: number; reason: string }>;
  action_taken: 'passed' | 'blocked' | 'redacted' | 'user_allowed' | 'user_blocked' | 'flagged';
  user_decision: ScanDecision | null;
  model_used: string;
  scan_duration_ms: number;
  data_class: DataClass;
  scanned_at_ms: number;
};

export type ProviderDescriptor = {
  id: string;
  name?: string;
  kind:
    | 'open-ai-compatible'
    | 'anthropic'
    | 'microsoft-foundry'
    | 'github-copilot'
    | 'ollama-local'
    | 'local-runtime'
    | 'mock';
  channel_class: 'public' | 'internal' | 'private' | 'local-only';
  capabilities: Array<'chat' | 'code' | 'vision' | 'embedding' | 'tool-use'>;
  models: string[];
  priority: number;
  available: boolean;
};

export type ModelRouterSnapshot = {
  providers: ProviderDescriptor[];
};

export type McpConnectionStatus = 'disconnected' | 'connecting' | 'connected' | 'error';
export type McpTransport = 'stdio' | 'sse' | 'streamable-http';

export type McpSandboxStatus = {
  active: boolean;
  source: 'per-server' | 'global' | 'none';
  allow_network: boolean;
  read_workspace: boolean;
  write_workspace: boolean;
  extra_read_paths: string[];
  extra_write_paths: string[];
};

export type McpServerSnapshot = {
  id: string;
  transport: McpTransport;
  channel_class: 'public' | 'internal' | 'private' | 'local-only';
  enabled: boolean;
  auto_connect: boolean;
  reactive: boolean;
  status: McpConnectionStatus;
  last_error: string | null;
  tool_count: number;
  resource_count: number;
  prompt_count: number;
  sandbox_status?: McpSandboxStatus;
};

export type McpToolUiMeta = {
  resource_uri?: string;
  visibility?: string[];
  csp?: {
    connect_domains?: string[];
    resource_domains?: string[];
    frame_domains?: string[];
    base_uri_domains?: string[];
  };
  permissions?: {
    camera?: unknown;
    microphone?: unknown;
    geolocation?: unknown;
    clipboard_write?: unknown;
  };
  prefers_border?: boolean;
};

export type McpToolInfo = {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  ui_meta?: McpToolUiMeta;
};

export type McpResourceInfo = {
  uri: string;
  name: string;
  description: string | null;
  mime_type: string | null;
  size: number | null;
};

export type McpPromptInfo = {
  name: string;
  description: string | null;
  arguments: Array<{ name: string; description: string | null; required: boolean | null }>;
};

export type McpNotificationEvent = {
  server_id: string;
  kind:
    | 'cancelled'
    | 'progress'
    | 'logging-message'
    | 'resource-updated'
    | 'resource-list-changed'
    | 'tool-list-changed'
    | 'prompt-list-changed';
  payload: unknown;
  timestamp_ms: number;
};

export type McpServerLog = {
  timestamp_ms: number;
  message: string;
};

export type McpServerLogEvent = {
  server_id: string;
  log: McpServerLog;
};

export type ToolDefinition = {
  id: string;
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  output_schema: Record<string, unknown> | null;
  channel_class: 'public' | 'internal' | 'private' | 'local-only';
  side_effects: boolean;
  approval: 'auto' | 'ask' | 'deny';
  annotations: {
    title: string;
    read_only_hint: boolean | null;
    destructive_hint: boolean | null;
    idempotent_hint: boolean | null;
    open_world_hint: boolean | null;
  };
};

export type ModelCapabilities = {
  tasks: string[];
  can_call_tools: boolean;
  has_reasoning: boolean;
  context_length: number | null;
  parameter_count: string | null;
};

export type InferenceParams = {
  context_length: number | null;
  max_tokens: number | null;
  temperature: number | null;
  top_p: number | null;
  repeat_penalty: number | null;
};

export type InstalledModel = {
  id: string;
  hub_repo: string;
  filename: string;
  runtime: string;
  capabilities: ModelCapabilities;
  status: string;
  size_bytes: number;
  local_path: string;
  sha256: string | null;
  installed_at: string;
  inference_params?: InferenceParams;
};

export type LocalModelSummary = {
  installed_count: number;
  total_size_bytes: number;
  models: InstalledModel[];
};

/** Pick the best human-readable display name for a local model.
 *  ONNX models have generic filenames (e.g. "onnx/model.onnx") so we
 *  show the hub repo instead.  GGUF/other models have descriptive filenames. */
export function displayModelName(model: InstalledModel): string {
  if (model.runtime === 'onnx') {
    return model.hub_repo || model.filename || model.id;
  }
  // For sharded safetensors the filename is just "model-00001-of-00005.safetensors"
  // which is meaningless to users — show the hub repo instead.
  if (model.filename && /^model-\d+-of-\d+\.\w+$/.test(model.filename)) {
    return model.hub_repo || model.filename || model.id;
  }
  return model.filename || model.id;
}

/** Whether a model is embedding-only (no generative inference params needed). */
export function isEmbeddingOnly(model: InstalledModel): boolean {
  const tasks = model.capabilities?.tasks ?? [];
  return tasks.length > 0 && tasks.every(t => t === 'embedding');
}

/** Map InstalledModel capabilities to provider CapabilityOption[] for the router. */
export function modelCapsToProviderCaps(model: InstalledModel): CapabilityOption[] {
  const caps: CapabilityOption[] = [];
  const tasks = model.capabilities?.tasks ?? [];
  for (const t of tasks) {
    if (t === 'chat' || t === 'text-generation') caps.push('chat');
    if (t === 'embedding') caps.push('embedding');
  }
  if (model.capabilities?.can_call_tools) caps.push('tool-use');
  // Deduplicate
  return [...new Set(caps)];
}

export type HubModelInfo = {
  id: string;
  author: string | null;
  last_modified: string | null;
  tags: string[];
  downloads: number;
  likes: number;
  pipeline_tag: string | null;
  library_name: string | null;
};

export type HubSearchResult = {
  models: HubModelInfo[];
  total: number;
};

export type HubFileInfo = {
  filename: string;
  size: number | null;
};

export type DownloadProgress = {
  model_id: string;
  repo_id: string;
  filename: string;
  total_bytes: number | null;
  downloaded_bytes: number;
  status: string;
  error: string | null;
};

export function parseModelMeta(model: HubModelInfo) {
  const tags = model.tags ?? [];
  const idLower = model.id.toLowerCase();

  // Format detection — prioritize specific formats (gguf/safetensors/onnx) over generic 'transformers'
  const specificFormats = ['gguf', 'safetensors', 'onnx'];
  const format = tags.find(t => specificFormats.includes(t))
    || (tags.includes('transformers') ? 'transformers' : null);

  // Runtime inference from format
  const runtime = format === 'gguf' ? 'llama-cpp'
    : format === 'onnx' ? 'onnx'
    : (format === 'safetensors' || format === 'transformers') ? 'candle'
    : null;

  // Quantization from model name or tags
  const quantMatch = model.id.match(/[_-](Q\d+_K(?:_[A-Z])?|Q\d+_\d|fp16|bf16|4bit|8bit|f16|f32|awq|gptq|exl2)/i);
  const quantization = quantMatch ? quantMatch[1].toUpperCase() : null;

  // Parameter count from model name (e.g. "1B", "7B", "13B", "70B", "1.5B", "0.5B")
  const paramMatch = model.id.match(/[_-](\d+(?:\.\d+)?)[Bb](?:[_-]|$)/);
  const paramCount = paramMatch ? paramMatch[1] + 'B' : null;

  // Capabilities
  const hasVision = model.pipeline_tag === 'image-text-to-text'
    || tags.some(t => ['vision', 'image-text-to-text', 'multimodal'].includes(t))
    || idLower.includes('vision') || idLower.includes('vlm');
  const hasToolUse = tags.some(t => t.includes('tool') || t.includes('function-calling'))
    || idLower.includes('tool-use') || idLower.includes('function-call');
  const hasReasoning = tags.some(t => t.includes('reasoning') || t.includes('chain-of-thought'))
    || idLower.includes('reasoning') || idLower.includes('-r1') || idLower.includes('deepseek-r');
  const isConversational = tags.includes('conversational');
  const isInstruct = idLower.includes('instruct') || idLower.includes('-it') || idLower.includes('-chat');

  // License
  const licenseTag = tags.find(t => t.startsWith('license:'));
  const license = licenseTag ? licenseTag.replace('license:', '') : null;

  // Languages
  const knownLangs = ['en', 'fr', 'de', 'es', 'it', 'pt', 'zh', 'ja', 'ko', 'ar', 'hi', 'th', 'ru', 'multilingual'];
  const languages = tags.filter(t => knownLangs.includes(t));

  // Architecture family from tags or model name
  const archTags = ['llama', 'mistral', 'gemma', 'phi', 'qwen', 'falcon', 'mpt', 'bloom', 'codellama', 'deepseek', 'yi', 'vicuna', 'starcoder'];
  let arch = tags.find(t => archTags.includes(t.toLowerCase())) || null;
  if (!arch) {
    for (const a of archTags) {
      if (idLower.includes(a)) { arch = a; break; }
    }
  }

  // Base model (for quantized models)
  const baseTag = tags.find(t => t.startsWith('base_model:') && !t.includes('quantized:'));
  const baseModel = baseTag ? baseTag.replace('base_model:', '') : null;
  const isQuantized = tags.some(t => t.includes('quantized:')) || !!quantization;

  return { format, runtime, quantization, paramCount, hasVision, hasToolUse, hasReasoning, isConversational, isInstruct, license, languages, arch, baseModel, isQuantized };
}

/** Extract quantization level from a filename (e.g. "model-Q4_K_M.gguf" → "Q4_K_M"). */
export function extractFileQuantization(filename: string): string | null {
  const match = filename.match(/[_.-](Q\d+_K(?:_[A-Z])?|Q\d+_\d|IQ\d+_[A-Z]+|fp16|bf16|4bit|8bit|f16|f32|awq|gptq|exl2)/i);
  return match ? match[1].toUpperCase() : null;
}

/**
 * An installable item in the install dialog.
 * For GGUF repos each file is self-contained; for safetensors repos,
 * sharded files are grouped into a single installable item.
 */
export type InstallableItem = {
  /** Display label shown to the user. */
  label: string;
  /** The filename to pass to the install API (first shard for grouped). */
  installFilename: string;
  /** Inferred runtime kind. */
  runtime: string;
  /** Total size in bytes (sum of all shards for grouped items). */
  totalSize: number | null;
  /** Number of files (1 for single, N for grouped shards). */
  fileCount: number;
  /** Quantization extracted from filename, if any. */
  quantization: string | null;
};

/**
 * Group raw HubFileInfo[] into InstallableItem[].
 * Only GGUF/GGML files are included (llama-cpp runtime for text generation).
 * Embedding-only formats (safetensors, ONNX) are excluded from the browse UI.
 */
export function groupInstallableFiles(files: HubFileInfo[]): InstallableItem[] {
  const items: InstallableItem[] = [];

  for (const f of files) {
    const lower = f.filename.toLowerCase();
    if (lower.endsWith('.gguf') || lower.endsWith('.ggml')) {
      items.push({
        label: f.filename,
        installFilename: f.filename,
        runtime: 'llama-cpp',
        totalSize: f.size,
        fileCount: 1,
        quantization: extractFileQuantization(f.filename),
      });
    }
  }

  return items;
}

export type HubRepoFilesResult = {
  repo_id: string;
  files: HubFileInfo[];
};

export type GpuInfo = {
  name: string;
  vendor: string;
  vram_bytes: number | null;
  driver_version: string | null;
};

export type CpuInfo = {
  name: string;
  cores_physical: number;
  cores_logical: number;
  arch: string;
};

export type MemoryInfo = {
  total_bytes: number;
  available_bytes: number;
};

export type HardwareInfo = {
  cpu: CpuInfo;
  memory: MemoryInfo;
  gpus: GpuInfo[];
};

export type HardwareSummary = {
  hardware: HardwareInfo;
  usage: {
    models_loaded: number;
    ram_used_bytes: number;
    vram_used_bytes: number;
    cpu_utilization: number;
  };
};

export type PerModelUsage = {
  model_id: string;
  memory_bytes: number;
};

export type RuntimeResourceUsage = {
  loaded_models: number;
  total_memory_used_bytes: number;
  per_model: PerModelUsage[];
};

export type KgNode = {
  id: number;
  node_type: string;
  name: string;
  data_class: DataClass;
  content: string | null;
};

export type KgEdge = {
  id: number;
  source_id: number;
  target_id: number;
  edge_type: string;
  weight: number;
};

export type KgNodeWithEdges = KgNode & {
  edges: KgEdge[];
};

export type KgTypeCount = {
  name: string;
  count: number;
};

export type KgStats = {
  node_count: number;
  edge_count: number;
  nodes_by_type: KgTypeCount[];
  edges_by_type: KgTypeCount[];
};

// ── Editable config types (match Rust serde output) ──────────────
export type PolicyAction = 'block' | 'prompt' | 'allow' | 'redact-and-send';
export type ScannerAction = 'block' | 'prompt' | 'flag' | 'allow';
export type ProviderKind = 'open-ai-compatible' | 'anthropic' | 'microsoft-foundry' | 'github-copilot' | 'ollama-local' | 'local-models' | 'mock';
export type CapabilityOption = 'chat' | 'code' | 'vision' | 'embedding' | 'tool-use';
export type ChannelClassOption = 'public' | 'internal' | 'private' | 'local-only';
export type RuntimeKind = 'candle' | 'onnx' | 'llama-cpp';
export type TransportKind = 'stdio' | 'sse' | 'streamable-http';

export type ProviderOptionsConfig = {
  route: string | null;
  allow_model_discovery: boolean;
  default_api_version: string | null;
  response_prefix: string | null;
  headers: Record<string, string>;
};

export type ModelProviderConfig = {
  id: string;
  name: string;
  kind: ProviderKind;
  base_url: string | null;
  auth: string;
  models: string[];
  capabilities?: CapabilityOption[];  // legacy, may be absent
  model_capabilities: Record<string, CapabilityOption[]>;
  channel_class: ChannelClassOption;
  priority: number;
  enabled: boolean;
  options: ProviderOptionsConfig;
};

export type ScannerModelEntry = { provider: string; model: string };

export type ScanSourceConfig = {
  workspace_files: boolean;
  clipboard: boolean;
  messaging_inbound: boolean;
  web_content: boolean;
  mcp_responses: boolean;
  tool_overrides: Record<string, boolean>;
};

export type CompactionStrategy = 'extract-and-summarize' | 'summarize-only' | 'manual';

export type ContextCompactionConfigData = {
  strategy: CompactionStrategy;
  trigger_threshold: number;
  keep_recent_turns: number;
  summary_max_tokens: number;
  extraction_model: string | null;
  max_summaries_in_context: number;
};

export type HiveMindConfigData = {
  daemon: { log_level: string; event_bus_capacity: number };
  api: { bind: string; http_enabled: boolean };
  security: {
    override_policy: { internal: PolicyAction; confidential: PolicyAction; restricted: PolicyAction };
    prompt_injection: { enabled: boolean; action_on_detection: ScannerAction; confidence_threshold: number; cache_ttl_secs: number; model_scanning_enabled?: boolean; scanner_models?: ScannerModelEntry[]; scan_sources?: ScanSourceConfig; max_payload_tokens?: number; batch_small_payloads?: boolean };
    default_permissions: Array<{ tool_pattern: string; scope: string; decision: string }>;
    sandbox?: { enabled: boolean; extra_read_paths: string[]; extra_write_paths: string[]; allow_network: boolean };
  };
  models: {
    providers: ModelProviderConfig[];
    request_timeout_secs?: number | null;
    stream_timeout_secs?: number | null;
  };
  local_models: { enabled: boolean; storage_path: string | null; max_loaded_models: number; max_download_concurrent: number; auto_evict: boolean };
  setup_completed: boolean;
  compaction: ContextCompactionConfigData;
  python?: { enabled: boolean; python_version: string; base_packages: string[]; auto_detect_workspace_deps: boolean; uv_version: string };
  node?: { enabled: boolean; node_version: string };
  afk?: {
    forward_on?: string[];
    forward_channel_id?: string | null;
    forward_to_address?: string | null;
    forward_approvals?: boolean;
    forward_questions?: boolean;
    auto_idle_after_secs?: number | null;
    auto_away_after_secs?: number | null;
    auto_approve_on_timeout_secs?: number | null;
  };
  web_search?: WebSearchConfig;
};

export type WebSearchConfig = {
  provider: string;
  api_key?: string | null;
};

// --- User Interaction types (question tool, tool approval) ---

export type InteractionKindToolApproval = {
  type: 'tool_approval';
  tool_id: string;
  input: string;
  reason: string;
};

export type InteractionKindQuestion = {
  type: 'question';
  text: string;
  choices: string[];
  allow_freeform: boolean;
  multi_select?: boolean;
  message?: string;
};

export type InteractionKind = InteractionKindToolApproval | InteractionKindQuestion;

export type UserInteractionRequest = {
  request_id: string;
  kind: InteractionKind;
};

export type InteractionResponseToolApproval = {
  type: 'tool_approval';
  approved: boolean;
  allow_session?: boolean;
};

export type InteractionResponseAnswer = {
  type: 'answer';
  selected_choice?: number | null;
  selected_choices?: number[];
  text?: string | null;
};

export type InteractionResponsePayload = InteractionResponseToolApproval | InteractionResponseAnswer;

export type UserInteractionResponse = {
  request_id: string;
  payload: InteractionResponsePayload;
};

// --- Agent Skills types ---

export type SkillManifest = {
  name: string;
  description: string;
  license?: string | null;
  compatibility?: string | null;
  metadata?: Record<string, string>;
  allowed_tools?: string | null;
};

export type DiscoveredSkill = {
  manifest: SkillManifest;
  source_id: string;
  source_path: string;
  installed: boolean;
};

export type SkillAuditRisk = {
  id: string;
  description: string;
  probability: number;
  severity: 'low' | 'medium' | 'high' | 'critical';
  evidence?: string | null;
};

export type SkillAuditResult = {
  model_used: string;
  risks: SkillAuditRisk[];
  summary: string;
  audited_at_ms: number;
};

export type InstalledSkill = {
  manifest: SkillManifest;
  local_path: string;
  source_id: string;
  source_path: string;
  audit: SkillAuditResult;
  enabled: boolean;
  installed_at_ms: number;
};

export type SkillSourceConfig =
  | { type: 'github'; owner: string; repo: string; enabled: boolean }
  | { type: 'local_directory'; path: string; enabled: boolean };

export type SkillsConfig = {
  enabled: boolean;
  sources: SkillSourceConfig[];
  storage_path?: string | null;
};

// ── Workspace file audit types ──────────────────────────────────

export type FileAuditStatus = 'unaudited' | 'safe' | 'risky' | 'stale';

export type RiskSeverity = 'low' | 'medium' | 'high' | 'critical';

export type FileAuditRisk = {
  id: string;
  description: string;
  probability: number;
  severity: RiskSeverity;
  evidence?: string | null;
};

export type FileAuditRecord = {
  path: string;
  content_hash: string;
  risks: FileAuditRisk[];
  verdict: 'clean' | 'suspicious' | 'detected';
  summary: string;
  model_used: string;
  audited_at_ms: number;
};

export type AuditStatusResponse = {
  record: FileAuditRecord | null;
  status: FileAuditStatus;
};

// ── Workspace classification types ──────────────────────────────

export type WorkspaceClassification = {
  default: DataClass;
  overrides: Record<string, DataClass>;
};

// ── Bot types ──────────────────────────────────────────────

export type BotMode = 'IdleAfterTask' | 'Continuous';

export interface BotConfig {
  id: string;
  friendly_name: string;
  description: string;
  avatar?: string | null;
  color?: string | null;
  model?: string | null;
  system_prompt: string;
  launch_prompt: string;
  allowed_tools: string[];
  data_class: DataClass;
  role: string;
  mode: BotMode;
  timeout_secs?: number | null;
  active: boolean;
  created_at: string;
  persona_id?: string | null;
}

export interface BotSummary {
  config: BotConfig;
  status: AgentStatus;
  last_error?: string | null;
  active_model?: string | null;
  tools: string[];
}

// ── Scheduler types ──────────────────────────────────────────────────

export type TaskStatus = 'pending' | 'running' | 'completed' | 'failed' | 'cancelled';

export interface TaskSchedule {
  type: 'once' | 'scheduled' | 'cron';
  run_at_ms?: number;
  expression?: string;
}

export interface PermissionRule {
  tool_pattern: string;
  scope: string;
  decision: 'auto' | 'ask' | 'deny';
}

export type TaskAction =
  | { type: 'send_message'; session_id?: string; content?: string }
  | { type: 'http_webhook'; url?: string; method?: string; body?: string; headers?: Record<string, string> }
  | { type: 'emit_event'; topic?: string; payload?: unknown }
  | { type: 'invoke_agent'; persona_id: string; task: string; friendly_name?: string; async_exec?: boolean; timeout_secs?: number; permissions?: PermissionRule[] }
  | { type: 'call_tool'; tool_id: string; arguments: any }
  | { type: 'composite_action'; actions: TaskAction[]; stop_on_failure?: boolean }
  | { type: 'launch_workflow'; definition: string; version?: string; inputs?: any; trigger_step_id?: string };

export interface ScheduledTask {
  id: string;
  name: string;
  description: string;
  schedule: TaskSchedule;
  action: TaskAction;
  status: TaskStatus;
  created_at_ms: number;
  updated_at_ms: number;
  last_run_ms?: number | null;
  next_run_ms?: number | null;
  run_count: number;
  last_error?: string | null;
  owner_session_id?: string | null;
  owner_agent_id?: string | null;
  max_retries?: number | null;
  retry_delay_ms?: number | null;
  retry_count?: number;
}

export type TaskRunStatus = 'success' | 'failure';

export interface TaskRun {
  id: string;
  task_id: string;
  started_at_ms: number;
  completed_at_ms?: number | null;
  status: TaskRunStatus;
  error?: string | null;
  result?: any | null;
}

// ── Communication Channels ──────────────────────────────────────────

export type ChannelType = 'microsoft' | 'gmail' | 'imap' | 'email' | 'slack' | 'discord' | 'telegram' | 'whatsapp';

export type ApprovalKind = 'auto' | 'ask' | 'deny';

export type DataClassification = 'public' | 'internal' | 'confidential' | 'restricted';

export interface DestinationRule {
  pattern: string;
  approval: ApprovalKind;
  input_class_override?: DataClassification | null;
  output_class_override?: DataClassification | null;
}

export interface MicrosoftConfig {
  from_address: string;
  folder: string;
  poll_interval_secs?: number | null;
  client_id: string;
  refresh_token?: string;
  access_token?: string | null;
}

export interface GmailConfig {
  from_address: string;
  folder: string;
  poll_interval_secs?: number | null;
  client_id: string;
  client_secret?: string | null;
  refresh_token?: string;
  access_token?: string | null;
}

export interface EmailConfig {
  imap_host: string;
  imap_port: number;
  smtp_host: string;
  smtp_port: number;
  auth: EmailAuth;
  from_address: string;
  folder?: string;
  poll_interval_secs?: number | null;
}

export type EmailAuth =
  | { type: 'oauth2'; provider: 'gmail' | 'outlook' | 'custom'; client_id: string; client_secret: string; refresh_token: string; access_token?: string | null; token_url?: string | null }
  | { type: 'password'; username: string; password: string };

export type ConnectorConfig =
  | { type: 'Microsoft'; config: MicrosoftConfig }
  | { type: 'Gmail'; config: GmailConfig }
  | { type: 'Email'; config: EmailConfig }
  | { type: 'Discord'; config: DiscordConfig }
  | { type: 'Slack'; config: SlackConfig }
  | { type: 'Stub'; config: { channel_type: string } };

export interface DiscordConfig {
  bot_token: string;
  allowed_guild_ids: string[];
  listen_channel_ids: string[];
  default_send_channel_id?: string | null;
}

export interface SlackConfig {
  bot_token: string;
  app_token: string;
  listen_channel_ids: string[];
  default_send_channel_id?: string | null;
  workspace_name?: string | null;
}

export interface DiscordGuild {
  id: string;
  name: string;
}

export interface DiscordChannel {
  id: string;
  name: string;
  type: number;
}

export interface SlackChannelInfo {
  id: string;
  name?: string | null;
  is_channel?: boolean;
  is_im?: boolean;
  is_member?: boolean;
}

export interface ChannelDiscoveryResponse {
  guilds?: DiscordGuild[];
  channels?: (DiscordChannel | SlackChannelInfo)[];
  workspace_name?: string;
  error?: string;
}

export interface ChannelConfig {
  id: string;
  name: string;
  channel_type: ChannelType;
  enabled: boolean;
  default_input_class: DataClassification;
  default_output_class: DataClassification;
  destination_rules: DestinationRule[];
  connector: ConnectorConfig;
  allowed_personas?: string[];
}

export interface ChannelInfo {
  id: string;
  name: string;
  channel_type: ChannelType;
  enabled: boolean;
  status: string;
}

export interface CommAuditEntry {
  id: string;
  channel_id: string;
  channel_type: string;
  direction: 'inbound' | 'outbound';
  from_address: string;
  to_address: string;
  subject?: string | null;
  body_hash: string;
  body_preview?: string | null;
  data_class: string;
  approval_decision?: string | null;
  agent_id?: string | null;
  session_id?: string | null;
  timestamp_ms: number;
}

// ── Workflow types ──────────────────────────────────────────────────

export type WorkflowStatus = 'pending' | 'running' | 'paused' | 'waiting_on_input' | 'waiting_on_event' | 'completed' | 'failed' | 'killed';

export type StepStatus = 'pending' | 'ready' | 'running' | 'completed' | 'failed' | 'skipped' | 'waiting_on_input' | 'waiting_on_event';

export type WorkflowMode = 'background' | 'chat';

export interface WorkflowDefinitionSummary {
  name: string;
  version: string;
  description: string | null;
  mode: WorkflowMode;
  trigger_types: string[];
  step_count: number;
  created_at_ms: number;
  updated_at_ms: number;
  bundled?: boolean;
  archived?: boolean;
  triggers_paused?: boolean;
  last_successful_run_at_ms?: number | null;
  is_untested?: boolean;
}

export interface WorkflowInstanceSummary {
  id: number;
  definition_name: string;
  definition_version: string;
  status: WorkflowStatus;
  parent_session_id: string;
  parent_agent_id: string | null;
  trigger_step_id?: string | null;
  created_at_ms: number;
  updated_at_ms: number;
  completed_at_ms: number | null;
  error: string | null;
  step_count?: number;
  steps_completed?: number;
  steps_failed?: number;
  steps_running?: number;
  has_pending_interaction?: boolean;
  pending_agent_approvals?: number;
  pending_agent_questions?: number;
  child_agent_ids?: string[];
  archived?: boolean;
  execution_mode?: 'normal' | 'shadow';
}

export interface StepState {
  step_id: string;
  status: StepStatus;
  started_at_ms: number | null;
  completed_at_ms: number | null;
  outputs: any | null;
  error: string | null;
  retry_count: number;
  child_workflow_id: number | null;
  child_agent_id: string | null;
  interaction_request_id: string | null;
  interaction_prompt?: string | null;
  interaction_choices?: string[] | null;
  interaction_allow_freeform?: boolean | null;
}

export interface WorkflowInstance {
  id: number;
  definition: any;
  status: WorkflowStatus;
  variables: any;
  step_states: Record<string, StepState>;
  parent_session_id: string;
  parent_agent_id: string | null;
  trigger_step_id?: string | null;
  permissions: any[];
  workspace_path: string | null;
  created_at_ms: number;
  updated_at_ms: number;
  completed_at_ms: number | null;
  output: any | null;
  error: string | null;
  execution_mode?: 'normal' | 'shadow';
}

// ── Workflow Impact Analysis types ───────────────────────────────────────

export interface EstimateRange {
  min: number;
  max: number | null;
  expression: string;
}

export interface ImpactTotals {
  external_messages: EstimateRange;
  http_calls: EstimateRange;
  agent_invocations: EstimateRange;
  destructive_ops: EstimateRange;
  scheduled_tasks: EstimateRange;
}

export interface StepRiskInfo {
  step_id: string;
  risk_level: 'safe' | 'caution' | 'danger' | 'unknown';
  action_summary: string;
  multiplier: string | null;
}

export interface WorkflowImpactEstimate {
  steps: StepRiskInfo[];
  totals: ImpactTotals;
  confidence: 'high' | 'medium' | 'low';
}

export interface WorkflowTestResult {
  test_name: string;
  passed: boolean;
  instance_id: number;
  failures: WorkflowTestFailure[];
  duration_ms: number;
  actual_status?: string;
  actual_output?: unknown;
  step_results?: StepStateSnapshot[];
  intercepted_actions?: InterceptedActionSnapshot[];
  intercepted_actions_total?: number;
}

export interface WorkflowTestFailure {
  expectation: string;
  expected: string;
  actual: string;
}

export interface StepStateSnapshot {
  step_id: string;
  status: string;
  outputs?: unknown;
  error?: string;
}

export interface InterceptedActionSnapshot {
  step_id: string;
  kind: string;
  details: Record<string, unknown>;
}

export interface WorkflowTestCase {
  name: string;
  description?: string;
  trigger_step_id?: string;
  inputs: Record<string, unknown>;
  shadow_outputs?: Record<string, unknown>;
  /** Per-step simulated tool calls (step_id → list of mock calls). */
  mock_tool_calls?: Record<string, MockToolCall[]>;
  expectations: TestExpectations;
}

export interface MockToolCall {
  tool_id: string;
  parameters?: Record<string, unknown>;
}

export interface TestExpectations {
  status?: string;
  output?: Record<string, unknown>;
  steps_completed?: string[];
  steps_not_reached?: string[];
  intercepted_action_counts?: Record<string, number>;
}

// ── Shadow / Intercepted Action types ────────────────────────────────────

export interface InterceptedAction {
  id: number;
  instance_id: number;
  step_id: string;
  kind: string; // "tool_call" | "agent_invocation" | "workflow_launch" | "scheduled_task" | "agent_signal" | "event_gate"
  timestamp_ms: number;
  details: Record<string, unknown>;
}

export interface InterceptedActionPage {
  items: InterceptedAction[];
  total: number;
}

export interface ShadowSummary {
  total_intercepted: number;
  tool_calls_intercepted: number;
  agent_invocations_intercepted: number;
  workflow_launches_intercepted: number;
  scheduled_tasks_intercepted: number;
  agent_signals_intercepted: number;
}

// ── Flight Deck types ────────────────────────────────────────────────────

export interface SystemHealthSnapshot {
  version: string;
  uptime_secs: number;
  pid: number;
  platform: string;
  active_session_count: number;
  active_agent_count: number;
  active_workflow_count: number;
  mcp_connected_count: number;
  mcp_total_count: number;
  total_llm_calls: number;
  total_input_tokens: number;
  total_output_tokens: number;
  knowledge_node_count: number;
  knowledge_edge_count: number;
  local_model_count: number;
  loaded_model_count: number;
}

export interface GlobalAgentEntry {
  agent_id: string;
  spec: AgentSpec;
  status: AgentStatus;
  last_error?: string | null;
  active_model?: string | null;
  tools: string[];
  parent_id?: string | null;
  started_at_ms?: number | null;
  session_id?: string | null;
  final_result?: string | null;
}

export interface SessionTelemetryEntry {
  session_id: string;
  title: string;
  state: ChatRunState;
  telemetry: TelemetrySnapshot;
}

// ── Services Dashboard types ─────────────────────────────────────────────

export type ServiceCategory = 'core' | 'connector' | 'mcp' | 'agents' | 'inference';
export type ServiceStatus = 'running' | 'stopped' | 'starting' | 'stopping' | 'error';

export interface ServiceSnapshot {
  id: string;
  display_name: string;
  category: ServiceCategory;
  status: ServiceStatus;
  last_error?: string | null;
}

export interface ServiceLogEntry {
  timestamp_ms: number;
  level: string;
  message: string;
  fields: Record<string, string>;
  target: string;
}

export interface ServiceStatusEvent {
  service_id: string;
  status: ServiceStatus;
  error?: string | null;
}

// ── Event Bus types ──────────────────────────────────────────────────────

export interface StoredEvent {
  id: number;
  event_id: number;
  topic: string;
  source: string;
  payload: any;
  timestamp_ms: number;
}

export interface EventTopic {
  topic: string;
  description: string;
  dynamic?: boolean;
}

// ── Active Trigger types ─────────────────────────────────────────────────

export interface ActiveTriggerSnapshot {
  definition_name: string;
  definition_version: string;
  trigger_kind: string;
  trigger_type: TriggerTypeInfo;
  next_run_ms: number | null;
}

/** Discriminated union matching Rust TriggerType (tagged by "type" field). */
export type TriggerTypeInfo =
  | { type: 'manual'; inputs?: any[]; input_schema?: any }
  | { type: 'incoming_message'; channel_id: string; listen_channel_id?: string; filter?: string; from_filter?: string; subject_filter?: string; body_filter?: string; mark_as_read?: boolean }
  | { type: 'event_pattern'; topic: string; filter?: string }
  | { type: 'mcp_notification'; server_id: string; kind?: string }
  | { type: 'schedule'; cron: string };

export interface ActiveEventGateSnapshot {
  subscription_id: string;
  instance_id: number;
  step_id: string;
  topic: string;
  filter: string | null;
  expires_at_ms: number | null;
}

export interface ActiveTriggersResponse {
  triggers: ActiveTriggerSnapshot[];
  event_gates: ActiveEventGateSnapshot[];
}
