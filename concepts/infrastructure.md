# Infrastructure Evaluation: Spatial Chat & Agent Stage

An assessment of what plumbing HiveMind OS needs to support the Spatial Chat and Multi-Agent Stage concepts described in `spatial-chat.md` and `agent-stage.md`.

---

## Current Architecture Baseline

| Layer | What exists today |
|-------|-------------------|
| **Frontend** | SolidJS single-page app, linear chat UI, settings panels |
| **Backend** | Axum HTTP server (`hive-api`), SQLite sessions, broadcast channels for streaming |
| **Agent loop** | `hive-loop` — YAML-driven workflow engine with ModelBackend/ToolBackend traits |
| **Model layer** | `hive-model` — ModelRouter with provider abstraction, streaming SSE |
| **Tools** | `hive-tools` — ToolRegistry with filesystem, shell, HTTP, calculator, etc. |
| **State** | Per-session in-memory HashMap + SQLite audit log; no persistent conversation graph |

---

## Gap Analysis

### 1. Canvas Data Model & Graph Store

**What's needed:** A persistent directed property graph for cards (nodes) and edges, with spatial coordinates, typed relationships, and metadata.

**Current gap:** Sessions are flat message lists (`Vec<ChatMessage>`) in a `HashMap`. There is no graph structure, no spatial index, no card typing.

**Required plumbing:**
- **Graph store abstraction** — A trait like `CanvasStore` with CRUD for nodes/edges, spatial queries (bounding box, radius), and graph traversal (connected components, BFS/DFS to depth N).
- **Spatial index** — R-tree implementation for O(log n) viewport culling and proximity queries. The `rstar` crate is the standard Rust R-tree.
- **Schema** — `CanvasNode` and `CanvasEdge` tables in SQLite (or a dedicated graph DB). Each node has `(id, type, x, y, width, height, content_json, metadata_json, created_by, status)`. Each edge has `(id, source, target, type, metadata_json)`.
- **Migration path** — Current `ChatMessage` must map to card nodes. A session becomes a canvas. Existing linear conversations can be auto-laid-out vertically.

**Effort estimate:** Medium-large. The graph store is new infrastructure but can be built on SQLite with JSON columns and an in-memory R-tree cache.

---

### 2. Real-Time Bidirectional Sync (CRDT / WebSocket)

**What's needed:** Multiple clients and agents concurrently editing the same canvas graph. Edits must merge without conflicts.

**Current gap:** The app uses HTTP request-response for all operations. The only real-time channel is a `broadcast::Sender<LoopEvent>` for streaming tokens — it's unidirectional (server→client) and not a general-purpose sync mechanism.

**Required plumbing:**
- **WebSocket transport** — Axum supports WebSocket upgrades natively. Need a persistent WS connection per client session.
- **CRDT layer** — For conflict-free concurrent edits. Options:
  - `yrs` (Yjs Rust port) — mature, battle-tested, supports maps/arrays/text.
  - `automerge-rs` — alternative with good Rust support.
  - Custom op-based CRDT for the graph (simpler: each node/edge is an independent LWW-Register).
- **Change propagation** — When agent emits a card, it writes to CRDT → CRDT broadcasts delta to all connected clients. When user drags a card, client writes position update to CRDT → server and other clients receive it.
- **Presence** — Cursor positions of all connected users, broadcast at ~10 Hz. Lightweight, doesn't need CRDT — just ephemeral pub/sub.

**Effort estimate:** Large. This is the single biggest infrastructure addition. A simpler v1 could use operational transforms over WebSocket without full CRDT (single-writer per node at any time).

---

### 3. Multi-Agent Orchestration

**What's needed:** Multiple agents running concurrently with different roles, models, contexts, and communication channels. A director (user) can broadcast, whisper, pause, redirect, and recast agents.

**Current gap:** `hive-loop` runs a single agent workflow per session. There is no concept of multiple concurrent agents, inter-agent messaging, or agent identity/roles.

**Required plumbing:**
- **Agent identity** — Each agent gets an ID, name, role, model binding, and system prompt. Defined in a struct like `AgentSpec { id, name, role, model, system_prompt, tools }`.
- **Agent supervisor** — A new orchestration layer above `WorkflowEngine` that manages N concurrent agents:
  - Spawn/pause/resume/kill individual agents
  - Route messages between agents (delegation edges)
  - Enforce budget limits per agent and globally
  - Track per-agent token usage and cost
- **Inter-agent messaging** — Agents can send artifacts/messages to other agents. This is essentially a channel between workflow engines. Could use `tokio::mpsc` channels with a routing table.
- **Topology definitions** — Pipeline, fan-out/fan-in, feedback loop, hierarchy, swarm. These can be expressed as YAML workflow configurations in the existing `hive-loop` schema by adding agent-reference action types.
- **Agent roster** — Registry of available agent types with their default configurations. Similar to the tool registry pattern.

**Effort estimate:** Large. The supervisor and inter-agent messaging are new. However, much of the single-agent plumbing (model backends, tool backends, state persistence) already exists in `hive-loop`.

---

### 4. Structured Agent Output (Card Emitter)

**What's needed:** Agents don't just return text — they emit structured card events: decomposition cards, tool-call cards, decision-point cards, synthesis cards, dead-end cards.

**Current gap:** `LoopEvent` is a flat enum with `Token`, `ModelDone`, `ToolCallStart`, `ToolCallResult`, `Done`, `Error`. It carries no spatial information, no card typing, no relationship metadata.

**Required plumbing:**
- **Extend WorkflowEvent** — Add card-oriented event variants:
  ```
  CardCreated { node: CanvasNode, edges: Vec<CanvasEdge> }
  CardUpdated { node_id, updates }
  CardStatusChanged { node_id, status: active|dead-end|archived }
  ```
- **Reasoning DAG builder** — A component that observes the workflow execution and automatically constructs the reasoning topology. When the engine:
  - Decomposes a task → emits a `decomposition` card with `decomposes-to` edges
  - Calls a tool → emits a `tool-call` card with `tool-io` edge to result
  - Makes a branch decision → emits a `decision-point` card
  - Abandons a path → marks the card as `dead-end`
  - Synthesizes results → emits a `synthesis` card with `synthesizes` edges
- **Layout algorithm** — Auto-position new cards relative to their parents. A simple tree layout (children fan out below parent) works for v1. Force-directed layout for organic clustering.

**Effort estimate:** Medium. The `WorkflowEvent` types already exist and can be extended. The reasoning DAG builder is a new observer that hooks into the workflow engine's event sink.

---

### 5. Context Assembly from Spatial Proximity

**What's needed:** When a user creates a prompt card at position (x, y), the system assembles LLM context from: (1) directly connected cards, (2) spatially nearby cards, (3) same-cluster cards, (4) graph-connected cards within N hops.

**Current gap:** Context assembly is currently prompt string + tool definitions. There is no spatial context model.

**Required plumbing:**
- **Context assembler** — A function `assemble_context(prompt_card, canvas_graph, token_budget) -> Vec<Message>` that:
  1. Queries the R-tree for nearby cards
  2. Walks graph edges for connected cards
  3. Prioritizes by layer (direct > proximity > cluster > graph)
  4. Truncates to fit the model's token budget
- **Token counting** — Need a fast token counter (tiktoken or similar) to budget context assembly. The `tiktoken-rs` crate works for OpenAI-compatible tokenizers. For local models, approximate by chars/4.
- **Active context region** — A UI concept where the user drags cards into/out of a visible region to control what the agent sees. Backend needs to track which cards are "in context" per agent.

**Effort estimate:** Medium. The assembler is algorithmic work. Token counting requires a new dependency. The R-tree query infrastructure comes from gap #1.

---

### 6. Frontend Canvas Rendering

**What's needed:** A 2D infinite canvas with pan/zoom, viewport culling, card rendering (DOM elements), edge rendering (SVG), and background effects (WebGL/Canvas2D).

**Current gap:** The frontend is a standard DOM-based SolidJS app with no canvas, no WebGL, no spatial rendering.

**Required plumbing:**
- **Canvas engine** — Options:
  - Build on `solid-pixi` or `solid-konva` for 2D rendering
  - Use a hybrid approach: WebGL background + SVG edges + DOM cards (as described in spatial-chat.md)
  - Consider existing canvas libraries: `tldraw` (React-based, would need port), `excalidraw`, or build custom
- **Viewport management** — Pan/zoom state, coordinate transforms (screen ↔ canvas), gesture handling (pinch zoom, scroll pan)
- **Culling system** — Only render cards visible in viewport + buffer. Use the R-tree spatial index (client-side mirror of server state)
- **Card components** — SolidJS components for each card type (prompt, response, artifact, tool-call, decomposition, decision-point, synthesis, dead-end) with expand/collapse, status indicators, and inline editing
- **Edge rendering** — SVG paths with animated particles for active data flows, typed styling (solid/dashed/red), labels

**Effort estimate:** Very large. This is the biggest frontend investment. Could be phased: v1 with simple DOM-based cards and static edges, v2 with full canvas rendering and animations.

---

### 7. Telemetry & Cost Tracking

**What's needed:** Per-agent and aggregate token usage, cost projection, budget fences, and a real-time telemetry dashboard.

**Current gap:** No token counting, no cost tracking, no budget enforcement. The `ModelResponse` metadata can carry token counts from providers but nothing aggregates or displays them.

**Required plumbing:**
- **Token/cost accumulator** — Track input/output tokens per model call. Map to cost using a provider-specific pricing table.
- **Budget fence** — A configurable dollar/token limit. When reached, pause all agents and emit a `BudgetExceeded` event.
- **Telemetry event stream** — Extend `WorkflowEvent` with `TokenUsage { input_tokens, output_tokens, cost }` events.
- **Frontend dashboard** — Collapsible panel showing per-agent costs, total cost, token velocity graph, and budget controls.

**Effort estimate:** Small-medium. Mostly data plumbing — the provider layer already returns token usage in some cases.

---

## Priority Ordering

Based on dependencies and impact:

| Priority | Component | Why |
|----------|-----------|-----|
| **P0** | Graph store + spatial index | Everything depends on this data model |
| **P0** | Structured agent output (card emitter) | Agents must produce cards, not just text |
| **P1** | Context assembly | The core value proposition of spatial chat |
| **P1** | Multi-agent orchestration | Required for agent stage |
| **P1** | Frontend canvas (v1) | Users must see and interact with cards |
| **P2** | WebSocket sync | Required for real-time updates and multi-user |
| **P2** | Telemetry & cost tracking | Important for agent stage but not blocking |
| **P3** | Full CRDT | Only needed for true multi-user collaboration |
| **P3** | Advanced canvas rendering | WebGL effects, animations, complex layouts |

---

## Existing Infrastructure That Can Be Leveraged

| Existing | Can support |
|----------|-------------|
| `hive-loop` WorkflowEngine | Single-agent execution; extend for multi-agent supervisor |
| `hive-loop` WorkflowEventSink | Hook for card emission and telemetry |
| `hive-loop` WorkflowStore trait | State persistence pattern; extend for canvas graph |
| `hive-model` ModelRouter | Multi-model routing; each agent can use different models |
| `hive-tools` ToolRegistry | Shared tool infrastructure across agents |
| `hive-api` Axum server | Add WebSocket upgrade routes alongside existing HTTP |
| `hive-api` broadcast channels | v1 real-time updates before full CRDT |
| SQLite (rusqlite) | Graph store backend with JSON columns |

---

## Recommended First Steps

1. **Define the `CanvasNode` / `CanvasEdge` Rust types** in `hive-contracts` — the shared data model that all layers will use.
2. **Build `CanvasStore` trait** in a new `hive-canvas` crate — SQLite-backed graph store with R-tree spatial index.
3. **Extend `WorkflowEventSink`** to emit `CardCreated` / `CardUpdated` events — this is the bridge between the agent loop and the canvas.
4. **Build a reasoning DAG observer** — watches workflow execution and auto-generates the card topology.
5. **Frontend v1** — Simple DOM-based card grid (not full canvas yet) that renders cards from the graph store. Validate the UX before investing in WebGL/hybrid rendering.
