# HIVEMIND OS CODEBASE ARCHITECTURE EXPLORATION

## 1. LOOP/MIDDLEWARE ARCHITECTURE (hive-loop)

### Main Agent Loop Entry Point: C:\dev\forge\crates\hive-loop\src\legacy.rs (2,716 lines)

#### Core Loop Execution Flow:
- LoopExecutor::run()  select strategy  strategy.run()
- Three strategy implementations: ReAct, Sequential, PlanThenExecute
- Each strategy yields LoopResult with final content, provider_id, model

#### LoopContext - Full Agent Context:
- session_id, message_id, prompt, history
- data_class (classification level)
- required_capabilities, preferred_models, role
- tools: Arc<ToolRegistry>
- permissions: Arc<Mutex<SessionPermissions>>
- skill_catalog, workspace_classification
- agent_orchestrator, conversation_journal
- tool_execution_mode (Parallel/SequentialFull/SequentialPartial)

#### Tool Call Structure:
- ToolCall { tool_id: String, input: Value }
- parse_tool_calls()  Multiple formats: XML <tool_call>, fenced code blocks, raw JSON
- Accepts multiple key names: "tool"/"tool_id"/"toolId"/"name"/"function"/"tool_name"
- Accepts multiple arg names: "input"/"arguments"/"parameters"/"params"/"args"

#### Middleware System - 4 Hooks:

`
pub trait LoopMiddleware: Send + Sync {
    fn before_model_call(&self, context: &LoopContext, request: CompletionRequest) 
        -> Result<CompletionRequest, LoopError>
    
    fn after_model_response(&self, context: &LoopContext, response: CompletionResponse)
        -> Result<CompletionResponse, LoopError>
    
    fn before_tool_call(&self, context: &LoopContext, call: ToolCall)
        -> Result<ToolCall, LoopError>
    
    fn after_tool_result(&self, context: &LoopContext, result: ToolResult)
        -> Result<ToolResult, LoopError>
}
`

#### Implemented Middleware:

**1. TokenBudgetMiddleware (C:\dev\forge\crates\hive-loop\src\token_budget.rs):**
   - Estimates tokens: ~1 token per 4 characters
   - Monitors input budget = context_window - output_reserve
   - Truncates: oldest history msgs (keep 6 recent)  prompt (oldest tool blocks first)
   - Errors if still over budget
   - Constants: KEEP_RECENT_MESSAGES=6, OUTPUT_RESERVE_FRACTION=0.15, OUTPUT_RESERVE_MIN=2048

**2. ContextCompactorMiddleware (C:\dev\forge\crates\hive-loop\src\compactor.rs):**
   - Triggers when token usage > threshold  context_window
   - Summarizes oldest non-system messages using model
   - Replaces with single summary message marked "[Compaction Summary"
   - Future: Extract to knowledge graph

#### Tool Call Processing Pipeline:

1. before_tool_call() hooks
2. Lookup tool in registry, get definition
3. infer_scope() extracts resource from tool_id + input
4. Session permission check: Auto/Ask/Deny
5. If Ask: UserInteractionGate.create_request()  await response
6. Tool execution via tool.execute()
7. after_tool_result() hooks
8. Format result into prompt: <tool_call>{...}</tool_call><tool_result>...</tool_result>

#### Tool Execution Modes:

`
ToolExecutionMode::Parallel           join_all futures concurrently
ToolExecutionMode::SequentialFull     sequential, continue on error
ToolExecutionMode::SequentialPartial  sequential, stop on first error
`

#### Loop Strategies:

**ReActStrategy:**
- Loop: model call  detect tool calls  execute batch  append results  continue
- Loop limit: MAX_TOOL_CALLS = 25
- Parse formats: XML, code fence, greedy JSON

**SequentialStrategy:**
- Single model call, no tool iterations
- Returns content immediately

**PlanThenExecuteStrategy:**
- First call: extract plan (numbered/dashed lines)
- Loop per plan step: execute up to MAX_TOOL_CALLS_PER_STEP=10
- Limits: MAX_PLAN_STEPS=10

#### Conversation Journal (Persist Tool Cycles):

- ConversationJournal::entries: Vec<JournalEntry>
- JournalPhase: ToolCycle | Plan{steps} | StepComplete{step_index, result}
- reconstruct_react_prompt() rebuilds full prompt from initial + all cycles
- Used for mid-task resume after crash

#### UserInteractionGate (Transport-agnostic):

- create_request(request_id, InteractionKind)  oneshot::Receiver
- respond(UserInteractionResponse)  sends via channel
- Supports: ToolApproval, Question interactions

#### Agent Orchestrator Trait:

- spawn_agent(), signal_agent(), message_session()
- feedback_agent(), list_agents(), get_agent_result()
- kill_agent(), search_bots()
- Enables agent spawning from within loops

---

## 2. MODERN WORKFLOW ENGINE

### Entry Point: C:\dev\forge\crates\hive-loop\src\engine.rs

#### WorkflowEngine:

`
pub struct WorkflowEngine {
    model: Arc<dyn ModelBackend>,
    tools: Arc<dyn ToolBackend>,
    store: Arc<dyn WorkflowStore>,
    events: Arc<dyn WorkflowEventSink>,
}

Methods:
- run(workflow, run_id, inputs)  Value
- resume(run_id)  Value
- run_builtin(name, run_id, inputs)  Value
`

#### WorkflowDefinition Schema (YAML):

`yaml
name: my-workflow
version: 1.0.0
config:
  max_iterations: 25
  max_tool_calls: 50
inputs:
  - name: user_input
    required: true
    default: null
steps:
  - id: step1
    action:
      type: model_call
      prompt: "{{ user_input }}"
      result_var: response
`

#### ActionDef Types:

- ModelCall { prompt, system_prompt, result_var }
- ToolCall { tool_name, arguments, result_var }
- Branch { condition, then_step, else_step }
- ReturnValue { value }
- SetVariable { name, value }
- Log { message, level }
- Loop { condition, max_iterations, steps }
- ParallelToolCalls { calls, result_var }

#### WorkflowState:

- run_id, workflow_name, status (Pending/Running/Completed/Failed/Paused)
- current_step, variables, messages, iteration_count, tool_call_count
- Timestamps: created_at, updated_at (ISO 8601)

#### Backend Traits (Zero hive-* dependencies):

`
pub trait ModelBackend: Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> WorkflowResult<ModelResponse>;
}

pub trait ToolBackend: Send + Sync {
    async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>>;
    async fn execute(&self, call: &ToolCall) -> WorkflowResult<ToolResult>;
}

pub trait WorkflowEventSink: Send + Sync {
    async fn emit(&self, event: WorkflowEvent);
}

pub trait WorkflowStore: Send + Sync {
    async fn save(&self, state: &WorkflowState) -> WorkflowResult<()>;
    async fn load(&self, run_id: &str) -> WorkflowResult<Option<WorkflowState>>;
    async fn delete(&self, run_id: &str) -> WorkflowResult<()>;
    async fn list_runs(&self) -> WorkflowResult<Vec<String>>;
}
`

#### InMemoryStore Implementation:

- HashMap<String, WorkflowState> behind Mutex
- Suitable for testing, data lost on drop

---

## 3. MCP INTEGRATION (hive-mcp)

### Types: C:\dev\forge\crates\hive-contracts\src\mcp.rs

#### Connection Management:

`
McpConnectionStatus: Disconnected | Connecting | Connected | Error

McpServerSnapshot {
    id, transport: McpTransportConfig, channel_class, enabled,
    status, last_error, tool_count, resource_count, prompt_count
}
`

#### Tool/Resource/Prompt Discovery:

`
McpToolInfo { name, description, input_schema: Value }
McpResourceInfo { uri, name, description, mime_type, size }
McpPromptArgumentInfo { name, description, required }
McpPromptInfo { name, description, arguments: Vec<McpPromptArgumentInfo> }
`

#### Notifications:

`
McpNotificationKind: Cancelled | Progress | LoggingMessage | ResourceUpdated | 
                     ResourceListChanged | ToolListChanged | PromptListChanged

McpNotificationEvent {
    server_id, kind, payload: Value, timestamp_ms
}
`

#### Service API:

- list_servers()  Vec<McpServerSnapshot>
- connect_server(id), disconnect_server(id)
- list_tools(id), call_tool(id, name, args)  McpCallToolResult
- list_resources(id), list_prompts(id)
- list_notifications(limit)

#### Transports:

- Stdio: Local subprocess (stdin/stdout)
- SSE: Remote server (HTTP)

#### Notification Queue:

- Capped at 200 events
- Collected async via event bus

---

## 4. SKILLS SERVICE (hive-skills-service)

### Main Service: C:\dev\forge\crates\hive-skills-service\src\lib.rs

#### SkillsService Operations:

`
pub struct SkillsService {
    index: Arc<SkillIndex>,      // SQLite: {data_dir}/skills.db
    config: RwLock<SkillsConfig>,
    cache_dir: PathBuf,          // {data_dir}/skills-cache/
    storage_dir: PathBuf,        // Installed skills: {storage_dir}/(name)/
    catalog: RwLock<Option<Arc<SkillCatalog>>>,
}

Methods:
- discover()  Query all sources  Vec<DiscoveredSkill>
- list_installed/discovered()
- audit_skill(source_id, source_path, content, model)  SkillAuditResult
- install_skill(name, source_id, source_path, audit)  InstalledSkill
- uninstall_skill(name)  bool
- set_skill_enabled(name, enabled)  bool
- rebuild_catalog()  Arc<SkillCatalog>
- fetch_skill_content(source_id, source_path)  SkillContent
`

#### Skill Types: C:\dev\forge\crates\hive-contracts\src\skills.rs

`
SkillManifest { name, description, license, compatibility, metadata, allowed_tools }

DiscoveredSkill { manifest, source_id, source_path, installed: bool }

InstalledSkill {
    manifest, local_path, source_id, source_path,
    audit: SkillAuditResult, enabled: bool, installed_at_ms: u64
}

SkillAuditResult {
    model_used: String, risks: Vec<SkillAuditRisk>,
    summary: String, audited_at_ms: u64
}

SkillAuditRisk {
    id: String, description, probability: f64,
    severity: Low|Medium|High|Critical, evidence: String
}
`

#### Directory Structure:

- {data_dir}/skills.db  SQLite index
- {data_dir}/skills-cache/  Downloaded caches
- {storage_dir}/(skill-name)/SKILL.md  Manifest + content
- {storage_dir}/(skill-name)/**  Supporting files

#### Source Discovery:

- SkillSourceConfig::GitHub { owner, repo, ... }
- GitHubRepoSource::discover()  Scan repos

#### Installation Flow:

1. discover() queries sources
2. audit_skill() LLM review (stub for now)
3. install_skill() parses SKILL.md, extracts files
4. rebuild_catalog() creates SkillCatalog for agents

---

## 5. CORE CONTRACTS (hive-contracts)

### Key Files: C:\dev\forge\crates\hive-contracts\src/

#### Tool Types: tools.rs

`
ToolApproval: Auto | Ask | Deny

ToolAnnotations {
    title, read_only_hint, destructive_hint,
    idempotent_hint, open_world_hint
}

ToolDefinition {
    id, name, description, input_schema, output_schema,
    channel_class, side_effects, approval, annotations
}
`

#### Interaction: interaction.rs

`
InteractionKind:
  - ToolApproval { tool_id, input, reason }
  - Question { text, choices, allow_freeform }

InteractionResponsePayload:
  - ToolApproval { approved, allow_session }
  - Answer { selected_choice, text }

UserInteractionRequest { request_id, kind }
UserInteractionResponse { request_id, payload }
`

#### Chat Events: chat.rs

`
SessionModality: Linear | Spatial

ReasoningEvent:
  StepStarted, ModelCallStarted/Completed, ToolCallStarted/Completed,
  BranchEvaluated, PathAbandoned, Synthesized, Completed, Failed,
  TokenDelta, UserInteractionRequired, QuestionAsked

trait ConversationModality: append_user_message, handle_reasoning_event, assemble_context
`

#### Configuration: config.rs

`
HiveMindConfig {
    daemon: DaemonConfig,
    api: ApiConfig,
    security: SecurityConfig,
    models: ModelsConfig,
    local_models: LocalModelsConfig,
    mcp_servers, hf_token, setup_completed,
    skills, personas, compaction, embedding
}

DaemonConfig { log_level, event_bus_capacity }
ApiConfig { bind: "127.0.0.1:9180", http_enabled }

SecurityConfig {
    override_policy: { internal, confidential, restricted },
    prompt_injection: PromptInjectionConfig,
    default_permissions: Vec<PermissionRule>
}

PolicyAction: Block | Prompt | Allow | RedactAndSend
`

#### Permissions: permissions.rs

`
SessionPermissions { resolve(tool_id, resource)  Option<ToolApproval> }
PermissionRule { tool_id, resource, approval }
infer_scope(tool_id, input)  Option<String>
`

---

## 6. RUNTIME WORKER (hive-runtime-worker)

### Entry: C:\dev\forge\crates\hive-runtime-worker\src\main.rs

`
ust
// Isolated process spawned per runtime type
// Supports: "candle", "onnx", "llama-cpp"

let runtime: Arc<dyn InferenceRuntime> = match cli.runtime.as_str() {
    "candle" => Arc::new(CandleRuntime::new()),
    "onnx" => Arc::new(OnnxRuntime::new()),
    "llama-cpp" => Arc::new(LlamaCppRuntime::new()),
    _ => { exit(1); }
};

run_worker_loop(runtime);
`

### Architecture:

- IPC protocol for communication
- Tracing to stderr only (stdout reserved for protocol)
- Panic hook logs before abort
- Event loop for continuous inference requests

### Related Modules:

- hive-inference/src/runtime.rs - Core trait
- hive-inference/src/runtime_candle.rs - Candle backend
- hive-inference/src/runtime_onnx.rs - ONNX backend
- hive-inference/src/runtime_llama.rs - llama.cpp backend
- hive-inference/src/worker_proxy.rs - IPC client
- hive-inference/src/worker_server.rs - IPC server

---

## SYSTEM CONSTANTS & LIMITS

| Constant | Value | File | Purpose |
|---|---|---|---|
| MAX_TOOL_CALLS | 25 | legacy.rs:1006 | ReAct iteration limit |
| MAX_PLAN_STEPS | 10 | legacy.rs:1007 | PlanThenExecute steps |
| MAX_TOOL_CALLS_PER_STEP | 10 | legacy.rs:1008 | Per-step limit |
| MAX_TOOL_OUTPUT_CHARS | 100,000 | legacy.rs:1013 | Output truncation (~25K tokens) |
| KEEP_RECENT_MESSAGES | 6 | token_budget.rs:48 | History msgs to preserve |
| OUTPUT_RESERVE_FRACTION | 0.15 | token_budget.rs:51 | Output headroom (% of context) |
| OUTPUT_RESERVE_MIN | 2048 | token_budget.rs:54 | Min output tokens |

---

## TOOL CALL FLOW DIAGRAM

Model Response
    
parse_tool_calls() [XML|fence|JSON formats]
    
execute_tool_batch() [Parallel|Sequential routes]
    
For each call:
  1. before_tool_call() middleware
  2. Lookup in ToolRegistry
  3. infer_scope()  resource
  4. SessionPermissions::resolve()  Auto|Ask|Deny
  5. If Ask: UserInteractionGate.create_request()  await response
  6. Tool execution
  7. after_tool_result() middleware
  8. Format: <tool_call>{...}<tool_result>
    
Append to prompt
    
ReAct loop continues or exits based on tool_calls present

---

## KEY INTERFACES

### ToolBackend (hive-loop/src/traits.rs)

`
ust
pub trait ToolBackend: Send + Sync {
    async fn list_tools(&self) -> WorkflowResult<Vec<ToolSchema>>;
    async fn execute(&self, call: &ToolCall) -> WorkflowResult<ToolResult>;
}
`

### ModelBackend (hive-loop/src/traits.rs)

`
ust
pub trait ModelBackend: Send + Sync {
    async fn complete(&self, request: &ModelRequest) -> WorkflowResult<ModelResponse>;
}
`

### LoopMiddleware (hive-loop/src/legacy.rs:348)

Four hooks: before_model_call, after_model_response, before_tool_call, after_tool_result

### WorkflowStore (hive-loop/src/store.rs)

`
ust
pub trait WorkflowStore: Send + Sync {
    async fn save(&self, state: &WorkflowState) -> WorkflowResult<()>;
    async fn load(&self, run_id: &str) -> WorkflowResult<Option<WorkflowState>>;
    async fn delete(&self, run_id: &str) -> WorkflowResult<()>;
    async fn list_runs(&self) -> WorkflowResult<Vec<String>>;
}
`

---

## EVENT BUS & EVENT LOG (hive-core)

### EventBus: `crates/hive-core/src/event_bus.rs`

The `EventBus` is a publish-subscribe system that distributes `EventEnvelope` messages (topic, payload JSON, timestamp, source) across the daemon.

**Two delivery modes:**

1. **Broadcast (lossy)** — `tokio::sync::broadcast` with configurable capacity (default 512). Subscribers that fall behind lose events (`RecvError::Lagged`). Used for ephemeral/fire-and-forget delivery (e.g., SSE streaming).

2. **Queued (lossless)** — Per-subscriber `tokio::sync::mpsc::unbounded` channels via the `QueuedSubscriber` trait. Each subscriber gets its own dedicated queue. Events are never dropped. Used for MCP notification watcher and the persistent event log.

**`QueuedSubscriber` trait:**
```
trait QueuedSubscriber: Send + Sync + 'static {
    fn accept(&self, envelope: &EventEnvelope) -> bool;  // topic filter
    fn send(&self, envelope: EventEnvelope);              // non-blocking
}
```

**Topic filtering:** `topic_matches_prefix(topic, prefix)` — matches exact topic or dot-separated children (e.g., prefix `"chat"` matches `"chat"` and `"chat.session.created"` but not `"chatbot"`).

### EventLog: `crates/hive-core/src/event_log.rs`

SQLite-backed persistent event log. Implements `QueuedSubscriber` — registered on the `EventBus` at daemon startup to durably store all events.

**Key features:**
- **Batch writer:** Internal mpsc channel → background task that flushes up to 64 events per batch for low publish-path latency.
- **Query API:** `query_events(topic_prefix, since_ms, limit)` — filter by topic prefix, time range, and count.
- **Retention:** `prune_before(timestamp_ms)` — delete old events to bound DB growth.
- **Recording sessions:** Start/stop named recordings that capture a time-bounded slice of events. Events are copied into `recording_events` table with `offset_ms` for replay timing.
- **Export:** JSON fixture files or Rust test scaffolds (`#[tokio::test]` functions) for reproducible integration tests.

**REST endpoints (hive-api):**
- `GET /api/v1/events` — query events
- `DELETE /api/v1/events` — prune old events
- `POST /api/v1/events/recordings` — start recording
- `POST /api/v1/events/recordings/{id}/stop` — stop recording
- `GET /api/v1/events/recordings` — list recordings
- `GET /api/v1/events/recordings/{id}` — get recording
- `GET /api/v1/events/recordings/{id}/export?format=json|rust_test` — export
- `DELETE /api/v1/events/recordings/{id}` — delete recording

---

END OF ARCHITECTURE EXPLORATION
