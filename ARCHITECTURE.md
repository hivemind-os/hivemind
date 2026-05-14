# HiveMind OS Architecture
> Audience: coding agents and new contributors.
> Scope: the architecture that exists in this repository today.
> Source basis: `crates\`, `apps\hivemind-desktop\`, and `packages\` as explored in code, not aspirational docs.
> Paths are written with Windows separators because that matches local development here.

## 1. Executive summary
HiveMind OS is a local-first agent platform centered on a daemon process.
At runtime the important stack is:
- `hive-daemon` boots the system.
- `hive-api` builds the Axum router and wires the service graph.
- `hive-chat` owns live chat sessions and the main interactive orchestration path.
- `hive-loop` contains the agentic chat loop (strategies, middleware, tool execution).
- `hive-model` routes LLM calls across providers.
- `hive-tools`, `hive-mcp`, `hive-plugins`, and `hive-connectors` provide the tool surface.
- `hive-workflow` provides workflow schema, persistence, and its own `WorkflowEngine`; `hive-workflow-service` integrates it with the runtime.
- the desktop app is a SolidJS frontend in a Tauri shell.

If you remember only four things, remember these:
- `hive-api` is the composition root.
- `hive-chat` is the main interactive runtime center of gravity.
- chat still uses `hive-loop::legacy`; workflows use `hive-workflow::WorkflowEngine` (separate from the loop crate).
- the runtime tool set is assembled dynamically per session and persona, not from one static global list.

## 2. Primary source files
Use these files first when re-orienting:
- `Cargo.toml` and `README.md` — workspace shape and repo framing.
- `crates\hive-daemon\src\main.rs` — daemon bootstrap, bind, auth token, shutdown.
- `crates\hive-api\src\lib.rs` — `AppState`, service wiring, router construction.
- `crates\hive-chat\src\chat.rs` — session runtime, tool assembly, turn processing.
- `crates\hive-loop\src\lib.rs` plus `crates\hive-loop\src\legacy\...` — loop split, chat executor, ReAct strategy, tool execution.
- `crates\hive-model\src\lib.rs` — `ModelRouter` and provider routing.
- `crates\hive-tools\src\lib.rs` — tool abstraction and registry.
- `crates\hive-mcp\src\lib.rs` and `session_mcp.rs` — daemon MCP service and per-session managers.
- `crates\hive-plugins\src\registry.rs`, `host.rs`, `protocol.rs`, `bridge.rs` — plugin lifecycle and bridge tools.
- `crates\hive-workflow\src\types.rs`, `store.rs`, and `crates\hive-workflow-service\src\lib.rs` — workflow schema, persistence, orchestration.
- `apps\hivemind-desktop\src\App.tsx`, `src\lib\authFetch.ts`, and `src-tauri\src\lib.rs` — frontend shell, daemon HTTP proxy, IPC/SSE bridge.
- `packages\plugin-sdk\README.md` — public plugin authoring model.

## 3. Repository layout
### 3.1 Top-level tree
- `apps\hivemind-desktop\`  desktop app.
- `crates\`  Rust workspace crates.
- `packages\plugin-sdk\`  TypeScript plugin SDK.
- `packages\plugin-registry\`  static plugin catalog metadata.
- `packages\sample-plugins\`  reference plugins.
- `packages\test-plugin\`  protocol/host API validation plugin.
- `packages\create-plugin\`  plugin scaffolder.
- `docs-site\`  hosted docs source.
- `tests\`  workspace-level tests.
### 3.2 Runtime tree under `.hivemind`
Important runtime paths include:
- `%USERPROFILE%\.hivemind\config.yaml`
- `%USERPROFILE%\.hivemind\run\`
- `%USERPROFILE%\.hivemind\audit.log`
- `%USERPROFILE%\.hivemind\knowledge.db`
- `%USERPROFILE%\.hivemind\risk-ledger.db`
- `%USERPROFILE%\.hivemind\local-models.db`
- `%USERPROFILE%\.hivemind\scheduler.db`
- `%USERPROFILE%\.hivemind\workflows.db`
- `%USERPROFILE%\.hivemind\workflow-attachments\`
- `%USERPROFILE%\.hivemind\plugins\plugins.json`
- `%USERPROFILE%\.hivemind\plugin-data\`
- `%USERPROFILE%\.hivemind\personas\...\persona.yaml`
- `%USERPROFILE%\.hivemind\personas\...\skills\...`
- `%USERPROFILE%\.hivemind\sessions\<session_id>\workspace`
- `%USERPROFILE%\.hivemind\sessions\<session_id>\.data_store.db`
- `%USERPROFILE%\.hivemind\canvas\<session_id>.db`
- `%USERPROFILE%\.hivemind\workflows\<generated-id>\workspace`

## 4. Runtime topology
```text
SolidJS frontend (`apps\hivemind-desktop\src`)
  -> Tauri IPC: invoke(...), listen(...), authFetch(...)
Tauri backend (`apps\hivemind-desktop\src-tauri`)
  -> authenticated HTTP + SSE to local daemon
hive-daemon
  -> builds AppState
  -> serves router from hive-api
hive-api
  -> hive-chat
  -> hive-mcp
  -> hive-local-models / hive-inference
  -> hive-scheduler
  -> hive-skills-service
  -> hive-workflow-service
  -> plugin registry + plugin host
hive-chat
  -> hive-loop::legacy
  -> hive-model
  -> hive-tools
  -> hive-mcp session manager
  -> hive-plugins bridge tools
  -> hive-knowledge
  -> hive-risk
  -> hive-context-map
  -> hive-workflow-service
```
The important architectural consequence is that `hive-api` is not a thin wrapper around libraries; it owns the service graph, while `hive-chat` owns the interactive execution path.

## 5. Layer model
### 5.1 Foundation
Crates: `hive-classification`, `hive-contracts`, `hive-core`, `hive-model`, `hive-risk`.
Responsibilities: shared DTOs/config, data classification and channel rules, audit/event bus, model/provider routing, prompt-injection and risk scanning.
### 5.2 Runtime/execution
Crates: `hive-inference`, `hive-process`, `hive-sandbox`, `hive-code-executor`, `hive-node-env`, `hive-python-env`, `hive-local-models`.
Responsibilities: local inference backends, managed subprocesses, sandboxing, CodeAct execution, Node/Python provisioning, local model lifecycle.
### 5.3 Orchestration
Crates: `hive-agent-kit`, `hive-agents`, `hive-loop`, `hive-scheduler`, `hive-workflow`, `hive-workflow-service`, `hive-skills`, `hive-skills-service`.
### 5.4 Integration/tool surface
Crates: `hive-connectors`, `hive-tools`, `hive-mcp`, `hive-plugins`, `hive-knowledge`, `hive-web-search`, `hive-workspace-index`, `hive-context-map`, `hive-canvas`, `hive-chat`.
### 5.5 Delivery
Crates/packages: `hive-api`, `hive-daemon`, `hive-cli`, `apps\hivemind-desktop\src-tauri`, `apps\hivemind-desktop\src`.

## 6. Workspace crate graph
### 6.1 Short graph
```text
hive-daemon
  -> hive-api
     -> hive-chat
        -> hive-loop (legacy chat loop)
        -> hive-model
        -> hive-tools
        -> hive-mcp
        -> hive-plugins
        -> hive-knowledge
        -> hive-risk
        -> hive-skills-service
        -> hive-workflow-service
     -> hive-mcp
     -> hive-local-models
     -> hive-scheduler
     -> hive-skills-service
     -> hive-workflow-service

hive-workflow-service
  -> hive-workflow
  -> hive-core
  -> hive-connectors

hive-tools
  -> hive-mcp
  -> hive-process
  -> hive-sandbox
  -> hive-scheduler
  -> hive-workflow
  -> hive-workflow-service
  -> hive-workspace-index

hive-plugins
  -> hive-node-env
  -> hive-tools
```
### 6.2 Direct internal deps by crate
> Only runtime (non-dev) dependencies are listed. Dev-dependencies are omitted.

| Crate | Role | Direct internal deps |
| --- | --- | --- |
| `hive-classification` | Data classification engine |  |
| `hive-contracts` | Shared DTOs and config types | `hive-classification` |
| `hive-core` | Config, paths, audit, event bus, daemon lifecycle | `hive-classification`, `hive-contracts` |
| `hive-model` | Model routing and provider abstraction | `hive-classification`, `hive-contracts`, `hive-core`, `hive-inference` |
| `hive-risk` | Prompt-injection and risk scanning | `hive-classification`, `hive-contracts`, `hive-core`, `hive-model` |
| `hive-inference` | Local inference runtimes | `hive-classification`, `hive-contracts`, `hive-core` |
| `hive-process` | Managed subprocess pool | `hive-sandbox` |
| `hive-sandbox` | Process isolation primitives |  |
| `hive-code-executor` | CodeAct execution backend |  |
| `hive-node-env` | Node runtime provisioning |  |
| `hive-python-env` | Python runtime provisioning |  |
| `hive-local-models` | Model download/install lifecycle | `hive-contracts`, `hive-inference` |
| `hive-agent-kit` | Agent-kit definitions/loading | `hive-contracts` |
| `hive-agents` | Agent orchestration/delegation | `hive-classification`, `hive-code-executor`, `hive-contracts`, `hive-loop`, `hive-model`, `hive-skills`, `hive-tools` |
| `hive-loop` | Legacy chat loop + workflow engine traits | `hive-classification`, `hive-code-executor`, `hive-connectors`, `hive-contracts`, `hive-core`, `hive-model`, `hive-risk`, `hive-skills`, `hive-tools` |
| `hive-scheduler` | Cron-based scheduling | `hive-contracts`, `hive-core` |
| `hive-workflow` | YAML workflow schema/persistence | `hive-contracts` |
| `hive-workflow-service` | Workflow orchestration service | `hive-connectors`, `hive-core`, `hive-workflow` |
| `hive-skills` | Skill definitions/indexes | `hive-contracts` |
| `hive-skills-service` | Skill installation/runtime service | `hive-contracts`, `hive-model`, `hive-skills` |
| `hive-connectors` | Connector infrastructure | `hive-classification`, `hive-contracts`, `hive-core` |
| `hive-tools` | Tool registry + built-in tools | `hive-classification`, `hive-connectors`, `hive-contracts`, `hive-mcp`, `hive-process`, `hive-sandbox`, `hive-scheduler`, `hive-workflow`, `hive-workflow-service`, `hive-workspace-index` |
| `hive-mcp` | MCP service and session bridge | `hive-classification`, `hive-contracts`, `hive-core`, `hive-node-env`, `hive-python-env`, `hive-sandbox` |
| `hive-plugins` | TypeScript plugin host | `hive-classification`, `hive-contracts`, `hive-node-env`, `hive-tools` |
| `hive-knowledge` | SQLite knowledge graph | `hive-classification` |
| `hive-web-search` | Web-search tool providers | `hive-classification`, `hive-contracts`, `hive-model`, `hive-tools` |
| `hive-workspace-index` | Workspace indexing/search | `hive-classification`, `hive-contracts`, `hive-inference`, `hive-iwork`, `hive-knowledge` |
| `hive-context-map` | Workspace-aware prompt enrichment | `hive-contracts`, `hive-model`, `hive-workspace-index` |
| `hive-canvas` | Spatial canvas storage and types | `hive-contracts` |
| `hive-chat` | Chat session orchestration | `hive-agents`, `hive-canvas`, `hive-classification`, `hive-code-executor`, `hive-connectors`, `hive-context-map`, `hive-contracts`, `hive-core`, `hive-inference`, `hive-iwork`, `hive-knowledge`, `hive-loop`, `hive-mcp`, `hive-model`, `hive-plugins`, `hive-process`, `hive-risk`, `hive-scheduler`, `hive-skills`, `hive-skills-service`, `hive-tools`, `hive-web-search`, `hive-workflow-service`, `hive-workspace-index` |
| `hive-api` | API and runtime composition | `hive-agent-kit`, `hive-agents`, `hive-canvas`, `hive-chat`, `hive-classification`, `hive-connectors`, `hive-contracts`, `hive-core`, `hive-inference`, `hive-knowledge`, `hive-local-models`, `hive-loop`, `hive-mcp`, `hive-model`, `hive-node-env`, `hive-plugins`, `hive-process`, `hive-python-env`, `hive-risk`, `hive-scheduler`, `hive-skills`, `hive-skills-service`, `hive-tools`, `hive-workflow`, `hive-workflow-service` |
| `hive-daemon` | Daemon binary | `hive-api`, `hive-classification`, `hive-core` |
| `hive-cli` | CLI for daemon/config control | `hive-core` |
| `hive-iwork` | Apple iWork document parsing | (external only) |
| `hive-runtime-worker` | Out-of-process inference worker binary | `hive-core`, `hive-inference` |
| `hive-test-utils` | Testing utilities and mock providers | `hive-api`, `hive-connectors`, `hive-contracts`, `hive-core`, `hive-model`, `hive-scheduler` |
### 6.3 Graph takeaways
- `hive-api` is the broadest dependency collector.
- `hive-chat` is the most central runtime crate after `hive-api`.
- `hive-tools` is the shared tool surface underneath chat, workflows, MCP, and plugins.

## 7. Runtime composition details
### 7.1 Daemon startup
In `crates\hive-daemon\src\main.rs` the daemon:
- loads config via `hive_core::load_config()`
- discovers/creates paths via `hive_core::discover_paths()` and `ensure_paths()`
- configures tracing and service log collection
- opens the audit log
- binds the HTTP listener before minting a new auth token
- writes `.hivemind\run\daemon.addr`
- builds `AppState`
- starts background services
- serves the router from `hive_api::build_router`
### 7.2 `AppState` as runtime hub
`crates\hive-api\src\lib.rs` defines `AppState`, which holds references to:
- config and paths
- audit logger and event bus
- chat service
- skills service
- MCP service and catalog
- local model service
- scheduler
- workflow service and trigger manager
- entity graph
- connector service
- plugin registry and plugin host
- Node/Python environment managers
- sandbox config and shell env
That is why major subsystem additions usually require touching `AppState` wiring even when the public API shape seems small.

## 8. Chat architecture
### 8.1 What `hive-chat` owns
`crates\hive-chat\src\chat.rs` owns:
- in-memory session records
- workspace creation per session
- session-local permissions
- session-local MCP managers
- persona resolution
- memory recall
- workspace context-map generation
- skill catalog selection
- model routing per turn
- per-session tool registry assembly
- event broadcasting to the UI
- question/approval interaction gates
- workflow-context injection into chat turns
- spatial canvas support
### 8.2 Session creation
When `create_session()` runs, the service:
- generates a session ID
- creates `.hivemind\sessions\<session_id>\workspace`
- seeds initial session permission rules, including workspace auto-grants
- creates a `ChatSessionSnapshot`
- creates `.hivemind\canvas\<session_id>.db` for spatial sessions
- builds a `SessionMcpManager` from the selected personas MCP configs
### 8.3 Important state fact
Live chat session state is primarily in `ChatService.sessions` in memory.
Durable state lives beside it in knowledge, workspaces, canvas DBs, audit logs, and workflow stores.
That means workflows and knowledge are more durable than an active interactive session.

## 9. Chat message flow
```text
App.tsx sendMessage()
  -> Tauri command `chat_send_message`
  -> POST /api/v1/chat/sessions/{session_id}/messages
  -> session route in hive-api
  -> `ChatService.enqueue_message()`
  -> worker spawn / `process_session()`
  -> model routing + tool assembly + loop execution
  -> broadcast/SSE events
  -> Tauri `listen(...)`
  -> SolidJS state update
```
### 9.1 `enqueue_message()` responsibilities
`ChatService.enqueue_message()` does much more than queue text. It:
- resolves the effective persona
- merges persona preferred models with per-message overrides
- classifies input into a `DataClass`
- calls the risk service for prompt-injection scanning
- may return `ReviewRequired` or `Blocked` before the turn starts
- creates the queued `ChatMessage`
- updates title, model selection, canvas position, and tool/skill exclusions
- may auto-answer a pending freeform question instead of starting a new turn
- may spawn a worker if the session is idle
### 9.2 `process_session()` responsibilities
For each queued turn `process_session()`:
- marks stages like classifying, recalling, routing, generating
- recalls memory from the knowledge layer
- rebuilds conversation history from session messages
- prepends spatial canvas context for spatial sessions
- prepends the persona system prompt
- appends a workspace context map
- appends workflow context when workflows are active for the session
- builds the skill catalog prompt
- builds multimodal prompt content from attachments
- routes the model with `ModelRouter`
- builds the session tool registry
- constructs `LoopContext`
- runs `LoopExecutor.run_with_events(...)`
- forwards loop events back to session streams, logs, approvals, question messages, and the spatial canvas observer

## 10. Tool architecture
### 10.1 The tool set is dynamic
`build_session_tools()` in `crates\hive-chat\src\chat.rs` constructs a `ToolRegistry` that can include:
- core built-in tools
- filesystem tools scoped to the session workspace
- shell/process tools
- HTTP and web-search tools
- connector-backed communication/calendar/drive/contact tools
- workflow tools
- a per-session SQLite data-store tool
- MCP tools from the catalog and session manager
- dynamically-discovered connector service tools
- plugin tools bridged from running plugins
After assembly it filters by persona `allowed_tools` and session `excluded_tools`.
Note: app-registered tools (MCP app iframes) are injected later during `process_session()`, not in `build_session_tools()`.
### 10.2 Core abstractions
`crates\hive-tools\src\lib.rs` re-exports and builds on the main tool abstractions (defined in `hive-contracts`):
- `Tool`
- `ToolDefinition` (defined in `hive-contracts`, re-exported here)
- `ToolRegistry`
- `ToolResult`
- `ToolError`
Common tool ID families include:
- `filesystem.*`
- `shell.execute`
- `workflow.*`
- `mcp.<server>.<tool>`
- `plugin.<plugin>.<tool>`
- `app.<instance>.<tool>`
### 10.3 Tool execution flow
```text
LLM emits tool call
  -> `hive-loop::legacy` parses it
  -> `legacy\tool_execution.rs::execute_tool_call`
  -> registry lookup
  -> canonical tool ID normalization
  -> session permission resolution
  -> connector destination-rule resolution
  -> channel/data-class policy check
  -> optional `UserInteractionGate` approval
  -> actual tool execution
  -> tool result + data class returned
```
### 10.4 Approval is layered
A tool can be denied or escalated because of:
- tool definition default approval
- session permission rules
- connector destination approval rules
- channel-class versus effective data-class mismatch
- connector outbound-class mismatch
- explicit user denial through the interaction gate
### 10.5 Tool listing for the UI
`crates\hive-api\src\routes\tools.rs` merges:
- base chat tools
- cataloged MCP tools
- running plugin tools
- session-only workflow/data-store/web-search tools
- then deduplicates by tool ID
So a missing tool in the UI may be a registration problem, a filtering problem, a plugin runtime problem, or an MCP catalog problem.

## 11. Loop architecture
This repo currently has two loop systems in one crate.
### 11.1 Chat path
Chat uses `hive-loop::legacy`.
Important pieces are:
- `LoopExecutor`
- `ReActStrategy`
- `SequentialStrategy`
- `PlanThenExecuteStrategy`
- `CodeActStrategy`
- `legacy\tool_execution.rs`
- `UserInteractionGate`
### 11.2 Workflow path
Workflow execution uses `hive-workflow::WorkflowEngine` (in `crates\hive-workflow\src\executor.rs`), not the loop crate.
`hive-loop` also exports generic workflow engine types and traits (`WorkflowEngine`, `WorkflowState`, `ModelBackend`, `ToolBackend`, `WorkflowStore` in `crates\hive-loop\src\engine.rs`, `state.rs`, `traits.rs`, `store.rs`), but these are not used by the product workflow path.
The product workflow runtime is:
- `hive-workflow` — `WorkflowEngine`, `WorkflowEvent`, schema, validation, persistence
- `hive-workflow-service` — product integration, event bus, chat injection, triggers
### 11.3 Contributor rule
- If you are changing chat behavior, start in `crates\hive-loop\src\legacy\...`.
- If you are changing workflow-engine behavior, start in `crates\hive-workflow\src\executor.rs` and `crates\hive-workflow-service\src\lib.rs`. The generic workflow engine in `hive-loop` (`engine.rs`, `actions.rs`, `traits.rs`) is a separate subsystem not used by the product workflow path.
Do not assume a loop change affects both systems.

## 12. Workflow architecture
### 12.1 Layers
There are two distinct pieces:
- `hive-workflow`  schema, validation, instance types, persistence, and `WorkflowEngine`
- `hive-workflow-service`  product integration, event bus, chat injection, attachments, workspace allocation, trigger management
### 12.2 Definition model
`crates\hive-workflow\src\types.rs` defines the workflow model.
Main trigger types:
- `manual`
- `incoming_message`
- `event_pattern`
- `mcp_notification`
- `schedule`
Main task step types:
- `call_tool`
- `schedule_task`
- `invoke_agent`
- `signal_agent`
- `feedback_gate`
- `event_gate`
- `launch_workflow`
- `delay`
- `set_variable`
- `invoke_prompt`
Main control-flow types:
- `branch`
- `for_each`
- `while`
- `end_workflow`
### 12.3 Persistence model
`crates\hive-workflow\src\store.rs` persists workflow state in SQLite.
Core tables include:
- `workflow_definitions`
- `workflow_definition_versions`
- `workflow_instances`
- `workflow_step_states`
- `trigger_dedup_v2`
- `cron_state_v2`
- `workflow_runtime_state`
- `workflow_intercepted_actions`
### 12.4 Product integration
`crates\hive-workflow-service\src\lib.rs` is responsible for:
- launching/resuming instances
- publishing workflow events onto the central `EventBus`
- auto-creating workflow workspaces under `.hivemind\workflows\...`
- exposing feedback gates to sessions
- injecting chat-mode workflow results back into chat history
- cleaning up workflow-owned child agents
- seeding bundled workflows into `workflows.db`
### 12.5 Workflow events
Important event topics include:
- `workflow.instance.created`
- `workflow.instance.started`
- `workflow.instance.completed`
- `workflow.instance.failed`
- `workflow.step.started`
- `workflow.step.completed`
- `workflow.step.waiting`
- `workflow.interaction.requested`
- `workflow.interaction.responded`

## 13. MCP architecture
### 13.1 Two MCP views
The MCP layer has:
- a daemon-level `McpService`
- a per-session `SessionMcpManager`
The global service manages configured servers and catalogs; the session manager provides the session-specific view used by chat and agent execution.
### 13.2 Where config lives
MCP servers are persona-owned.
`Persona` in `crates\hive-contracts\src\config.rs` has `mcp_servers: Vec<McpServerConfig>`.
`McpServerConfig` includes:
- `id`
- `transport`
- `command` / `args`
- `url`
- `env`
- `headers`
- `channel_class`
- `enabled`
- `auto_connect`
- `reactive`
- `auto_reconnect`
- optional `sandbox`
Current transport kinds are `stdio`, `sse`, and `streamable_http`.
### 13.3 Runtime path
At startup and config reconciliation:
- `hive-api` collects MCP configs across personas
- updates the global `McpService`
- refreshes the MCP catalog
At session creation:
- `hive-chat` resolves the selected persona
- calls `mcp_configs_for_persona(...)`
- builds a `SessionMcpManager`
- attaches the workspace path to that manager
At tool assembly:
- `build_session_tools()` calls `register_mcp_tools(...)`
- tool IDs become `mcp.<server_id>.<tool_name>`
### 13.4 Desktop support
The frontend has dedicated MCP support in:
- `apps\hivemind-desktop\src\components\PersonasTab.tsx`
- `McpServerWizard.tsx`
- `McpRegistryBrowser.tsx`
- `App.tsx` MCP state/signals
### 13.5 MCP Apps
MCP tools with UI metadata can surface an embedded HTML app.
The bridge is:
- the app UI registers tools via `/api/v1/mcp/app-tools/register`
- the daemon requests an app-tool invocation
- the frontend routes it to the correct iframe bridge
- the frontend answers with `/api/v1/mcp/app-tools/respond`
Those tools are then exposed back to the agent as normal session tools through `AppToolProxy`.

## 14. Plugin architecture
### 14.1 What plugins are
Plugins are TypeScript processes hosted by the daemon.
They are distinct from MCP servers.
The implementation in `crates\hive-plugins\src\...` uses JSON-RPC 2.0 over stdio with a Node child process per running plugin.
### 14.2 Main pieces
- `PluginRegistry`  installed plugin metadata persisted to `plugins.json`
- `PluginHost`  spawns and manages plugin processes
- `protocol.rs`  host/plugin method definitions
- `PluginBridgeTool`  adapts plugin tools into normal `ToolDefinition`s
- `PluginMessageRouter`  routes plugin-emitted messages into the connector layer
### 14.3 Lifecycle
```text
register or install plugin
  -> persist metadata in `plugins.json`
  (plugin is NOT spawned at install time)

on daemon startup or when plugin is enabled:
  -> spawn Node child process
  -> send `initialize`
  -> activate plugin
  -> optionally start background loop
  -> list tools
  -> bridge tools as `plugin.<plugin_id>.<tool>`
  -> expose them through the normal ToolRegistry
```
### 14.4 Host APIs
Plugin host APIs cover:
- secrets
- persistent KV store
- logging
- notifications
- events
- scheduling
- proxied HTTP
- file/data-dir operations
- persona listing
- connector listing
### 14.5 Install surfaces
The API exposes routes for:
- listing plugins
- getting config schema
- saving config
- enabling/disabling
- persona scoping
- uninstalling
- linking a local dev plugin
- installing from npm
The desktop connector flow also consumes `packages\plugin-registry\registry.json` as a catalog source.

## 15. Knowledge, search, and indexing
### 15.1 Knowledge graph
`hive-knowledge` provides a SQLite-backed property graph with vector search.
Important facts:
- it uses `sqlite-vec`
- it is pooled through `KgPool`
- writes are serialized with a semaphore
- chat memory recall and workspace-aware features reuse it
### 15.2 Workspace index
`hive-workspace-index` sits on top of workspace files, embeddings from `hive-inference`, knowledge storage, and specialized parsers such as `hive-iwork`.
### 15.3 Context maps
`hive-context-map` generates workspace summaries that `hive-chat` appends as extra system context according to the selected persona strategy.

## 16. Shared type boundaries
| Type area | Important types | Main source |
| --- | --- | --- |
| Config/topology | `HiveMindConfig`, `HiveMindPaths`, `Persona`, `PromptTemplate`, `McpServerConfig`, `McpTransportConfig` | `crates\hive-contracts\src\config.rs` |
| Chat boundary | `ChatMessage`, `ChatSessionSummary`, `ChatSessionSnapshot`, `SessionModality`, `ReasoningEvent`, `SendMessageRequest`, `SendMessageResponse` | `crates\hive-contracts\src\chat.rs` |
| Tool boundary | `ToolDefinition`, `ToolDefinitionBuilder`, `ToolApproval`, `SessionPermissions`, `PermissionRule` | `crates\hive-contracts\src\tools.rs`, `permissions.rs` |
| Interaction boundary | `UserInteractionRequest`, `InteractionKind`, `UserInteractionResponse`, `InteractionResponsePayload` | `crates\hive-contracts\src\interaction.rs` |
| MCP boundary | `McpServerSnapshot`, `McpToolInfo`, `McpPromptInfo`, `McpResourceInfo`, `McpCatalogEntry`, `McpNotificationEvent`, `McpAppResource` | `crates\hive-contracts\src\mcp.rs` |
| Workflow boundary | `TaskDef`, `ControlFlowDef`, `TriggerDef`, `TriggerType`, `WorkflowInstance`, `WorkflowStatus`, `StepState`, `WorkflowEvent`, `ModelBackend`, `ToolBackend` | `crates\hive-workflow\src\types.rs`, `crates\hive-loop\src\traits.rs` |
| Skills/risk | `InstalledSkill`, `RiskScanRecord` | `crates\hive-contracts\src\skills.rs`, `risk.rs` |
Important nuance: `InteractionKind` currently covers `ToolApproval`, `Question`, and `AppToolCall`, while workflow feedback gates are surfaced separately through the API interaction aggregator as `workflow_gate` entries.

## 17. Storage model
### 17.1 Default home
By default HiveMind uses `%USERPROFILE%\.hivemind\` as `hivemind_home`, derived in `crates\hive-core\src\config.rs`.
### 17.2 Main runtime files and databases
| Path | Owner | Purpose |
| --- | --- | --- |
| `.hivemind\config.yaml` | `hive-core` | merged user config |
| `.hivemind\run\daemon.addr` | `hive-daemon` | actual bound daemon address |
| `.hivemind\run\hive-daemon.pid` | `hive-daemon` | PID file |
| `.hivemind\audit.log` | `hive-core` | append-only audit log |
| `.hivemind\knowledge.db` | `hive-knowledge` | long-term graph memory + vector search |
| `.hivemind\risk-ledger.db` | risk layer | stored risk-scan records |
| `.hivemind\local-models.db` | `hive-local-models` | model install/download metadata |
| `.hivemind\scheduler.db` | scheduler service | scheduled tasks and runs |
| `.hivemind\workflows.db` | workflow store | definitions, instances, step states, trigger state |
| `.hivemind\skills.db` | `hive-skills-service` | installed/discovered skill metadata |
| `.hivemind\plugins\plugins.json` | `hive-plugins` | plugin registry state |
| `.hivemind\plugin-data\<plugin>` | `hive-plugins` | plugin-scoped data |
| `.hivemind\personas\<ns>\persona.yaml` | persona loader | persona definitions |
| `.hivemind\personas\<persona>\skills\<skill>` | skills service | installed skill contents |
| `.hivemind\sessions\<session_id>\workspace` | `hive-chat` | per-session working directory |
| `.hivemind\sessions\<session_id>\.data_store.db` | `DataStoreTool` | per-session SQLite scratchpad |
| `.hivemind\canvas\<session_id>.db` | `hive-canvas` | spatial session canvas store |
| `.hivemind\workflows\<id>\workspace` | workflow service | auto-created workflow workspace |
| `.hivemind\workflow-attachments\...` | workflow service | uploaded workflow attachments |
### 17.3 Durable versus in-memory state
Durable:
- config
- personas
- knowledge
- workflow definitions/instances
- skills
- plugin registry
- workspace files
- audit/risk ledgers
- local model metadata
Primarily in memory:
- live `ChatService.sessions`
- active broadcast channels
- live loop execution state
- active plugin processes
- Tauri-side subscription handles

## 18. Desktop app architecture
### 18.1 Split architecture
The desktop app is split into:
- `apps\hivemind-desktop\src`  SolidJS frontend
- `apps\hivemind-desktop\src-tauri`  Rust Tauri backend
### 18.2 Navigation model
The app is primarily screen-state driven, not router-driven.
`App.tsx` uses:
- `activeScreen`: `session`, `bots`, `scheduler`, `workflows`, `settings`, `agent-kits`
- `activeTab`: `chat`, `workspace`, `stage`, `workflows`, `events`, `processes`, `config`, `mcp`
So new top-level screens usually mean editing the `activeScreen` union, nav UI, and `Match` branches in `App.tsx`.
### 18.3 Communication patterns
The frontend talks to the backend through:
- `invoke(...)` for most command-style calls
- `listen(...)` for Tauri event streams
- `authFetch(...)` for daemon HTTP APIs proxied through Tauri
`authFetch()` exists because production Tauri runs under `https://tauri.localhost`, so direct browser `fetch()` to `http://127.0.0.1:*` would be mixed content.
### 18.4 SSE forwarding model
The Tauri backend keeps long-lived SSE subscriptions to daemon endpoints and republishes them into Tauri events.
Examples in `apps\hivemind-desktop\src-tauri\src\lib.rs` include streams for:
- chat session events
- interaction snapshots
- workflow events
- MCP events
- event-bus streams
- agent-stage streams
- workspace index-status streams
### 18.5 Frontend state shape
`App.tsx` is still the frontend composition root. It owns high-level state for:
- daemon context/status
- sessions and selected session
- personas and selected agent
- MCP state
- tool definitions
- installed skills
- risk scans
- model router snapshot
- screen/tab selection
Some state has been extracted into stores, including:
- `workflowStore.ts`
- `interactionStore.ts`
- `workspaceStore.tsx`
### 18.6 Interaction UX model
The interaction UX is a push+poll hybrid.
`interactionStore.ts`:
- attaches `listen("interaction:event")`
- asks Tauri to subscribe to the daemon interaction SSE stream
- polls `list_pending_interactions` and `get_pending_interaction_counts` as a safety net
Frontend-side interaction routing is centralized in `apps\hivemind-desktop\src\lib\interactionRouting.ts`, which routes by backend-provided `routing` and `type` rather than guessing.

## 19. TypeScript package structure
- `packages\plugin-sdk` — `definePlugin(...)`, Zod-based config schemas, tool definitions, loops, lifecycle hooks, test harnesses, and schema extraction to `dist\config-schema.json`.
- `packages\plugin-registry` — static JSON catalog consumed by desktop discovery flows.
- `packages\test-plugin` — protocol/host API validation plugin.
- `packages\sample-plugins\github-issues` — best in-repo reference plugin for real config/tool/loop structure.
- `packages\create-plugin` — scaffolds new plugin projects.

## 20. How to extend the system
### 20.1 Add a new built-in tool
Typical path:
1. Implement a new `Tool` in `crates\hive-tools\src\...`.
2. Give it a stable `ToolDefinition` with the right `id`, `channel_class`, `approval`, annotations, and JSON schema.
3. Register it in `build_session_tools()` if chat sessions should see it.
4. For UI/workflow-authoring visibility, also ensure it appears through the tool listing in `routes\tools.rs` (tools registered only in `build_session_tools()` are available at runtime but not automatically listed in `/api/v1/tools`).
5. Decide whether it is read-only or side-effecting, persona-filterable, session-permission-aware, and subject to extra data-class or connector rules.
6. If it needs special approval or policy behavior, inspect `crates\hive-loop\src\legacy\tool_execution.rs`.
Primary files:
- `crates\hive-tools\src\...`
- `crates\hive-chat\src\chat.rs` (`build_session_tools`)
- `crates\hive-loop\src\legacy\tool_execution.rs`
- `crates\hive-api\src\routes\tools.rs`
### 20.2 Add a new workflow definition
For a normal workflow you often do not need new Rust code.
Typical path:
1. Create/edit YAML through the workflow editor or API/Tauri commands.
2. The desktop frontend uses commands such as `workflow_list_definitions`, `workflow_get_definition`, and `workflow_save_definition`.
3. The backend persists the definition into `workflows.db`.
4. `WorkflowService` launches and manages instances from that stored definition.
If you want a bundled workflow that ships with the product:
1. add the YAML to the bundled workflow source in core
2. ensure it validates against `WorkflowDefinition`
3. let `WorkflowService::seed_bundled_workflows()` seed or auto-update it
### 20.3 Add a new workflow step type
Backend work usually means:
1. add a variant to `TaskDef`, `TriggerType`, or `ControlFlowDef`
2. update validation in `crates\hive-workflow\src\validation.rs`
3. update dispatch/execution in `crates\hive-workflow\src\executor.rs`
4. update persistence/runtime state if the new step needs new stored fields
5. update workflow-service integration if the step touches chat, triggers, attachments, or scheduling
Desktop work usually means:
1. add the palette item in `WorkflowDesigner.tsx`
2. add default config and required-field rules
3. add editor UI in `src\components\workflow\StepEditor.tsx` and related workflow files
4. verify YAML serialization/deserialization
### 20.4 Add a new MCP server
If you are adding another configured server using existing transports:
1. add an `McpServerConfig` to a personas `mcp_servers` in `persona.yaml` or through the Personas UI
2. config save/reconcile updates the daemon-level `McpService`
3. sessions using that persona build a `SessionMcpManager` from the new config
4. the session tool registry exposes its tools as `mcp.<server>.<tool>`
If you are adding a brand-new transport capability:
1. extend `McpTransportConfig`
2. teach `hive-mcp` how to connect using that transport
3. update config validation
4. update desktop `types.ts` and the MCP wizard UI
Primary files:
- `crates\hive-contracts\src\config.rs`
- `crates\hive-api\src\lib.rs`
- `crates\hive-chat\src\chat.rs`
- `apps\hivemind-desktop\src\components\PersonasTab.tsx`
- `apps\hivemind-desktop\src\components\McpServerWizard.tsx`
### 20.5 Add a new plugin
Typical path:
1. scaffold with `packages\create-plugin` or start from `packages\sample-plugins\github-issues`
2. implement `definePlugin(...)` with config schema, tools, and optional loop
3. build compiled JS plus `dist\config-schema.json`
4. install via local link or npm
5. let the daemon persist it in `plugins.json`
6. let the plugin host spawn it and bridge its tools into session registries
Product-facing plugin changes usually involve:
- `crates\hive-plugins\src\...`
- plugin API routes in `hive-api`
- desktop discovery/config flows
- optionally `packages\plugin-registry`
### 20.6 Add a new desktop screen
Typical path:
1. add a new screen ID to `activeScreen` in `App.tsx`
2. add navigation UI for it
3. add a new `Match` branch for rendering
4. create a component and, if useful, a dedicated store
5. use `invoke(...)`, `authFetch(...)`, or a new Tauri SSE subscription for its data needs
6. add new Tauri commands or daemon routes if the existing bridge surface is not enough
### 20.7 Add a new approval or interaction type
This is cross-cutting.
At minimum think about five layers:
1. shared contracts  update `crates\hive-contracts\src\interaction.rs`
2. producer side  update the chat loop, app-tool bridge, or workflow service that emits the interaction
3. API aggregation  update `crates\hive-api\src\routes\interactions.rs` if it should appear in the unified pending-interactions view
4. Tauri/frontend routing  update `interactionRouting.ts`, `interactionStore.ts`, relevant UI, and possibly `src-tauri\src\lib.rs`
5. AFK/offline handling  inspect `crates\hive-api\src\afk.rs` if the interaction should work away from the desktop
Important nuance: today there are two slightly different models:
- `InteractionKind` for chat-loop/app-tool interactions
- `workflow_gate` entries synthesized by the API layer from workflow service state
So first decide whether the new interaction belongs in the interaction-gate protocol, the workflow pending-gate system, or both.

## 21. Common architectural pitfalls
- Do not assume chat uses the new workflow engine; it still uses `hive-loop::legacy`.
- Do not assume tool availability is global and static; it depends on persona, exclusions, MCP, plugins, and connector discovery.
- Do not assume all pending interactions use the same type model; workflow gates are aggregated separately.
- Do not assume the desktop app talks directly to the daemon with browser `fetch`; production HTTP is proxied through `authFetch()` -> `daemon_fetch`.
- Do not assume active chat session state is fully durable; much of it is runtime memory.

## 22. Where to start for common tasks
- Change chat behavior: `crates\hive-chat\src\chat.rs`, `crates\hive-loop\src\legacy\...`, `crates\hive-model\src\lib.rs`
- Add/modify a tool: `crates\hive-tools\src\...`, `crates\hive-chat\src\chat.rs`, `crates\hive-loop\src\legacy\tool_execution.rs`
- Change workflow execution: `crates\hive-workflow\src\types.rs`, `executor.rs`, `crates\hive-workflow-service\src\lib.rs`, `apps\hivemind-desktop\src\components\WorkflowDesigner.tsx`
- Change plugin behavior: `crates\hive-plugins\src\registry.rs`, `host.rs`, `protocol.rs`, `bridge.rs`, `packages\plugin-sdk\README.md`
- Change desktop navigation/screens: `apps\hivemind-desktop\src\App.tsx`, relevant store, `apps\hivemind-desktop\src-tauri\src\lib.rs`
- Change runtime wiring: `crates\hive-daemon\src\main.rs`, `crates\hive-api\src\lib.rs`
