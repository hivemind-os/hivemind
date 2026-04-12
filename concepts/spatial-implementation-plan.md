# Implementation Plan: Chat Modalities, Spatial Chat & Agent Stage

## Abstraction Review & Corrections

The initial draft of this plan had several places where implementation details leaked across abstraction boundaries. This section documents the problems and the corrected architecture.

### Problem 1: AgentEvent contains CanvasNode — agent loop knows about canvas

**Leak:** `AgentEvent::CardCreated { node: CanvasNode, ... }` means the agent loop emits spatial types (x, y, width, height). The agent loop should not know which modality it's running in.

**Fix:** Split into two layers:
- **ReasoningEvent** (semantic, emitted by agent loop) — describes *what happened* during reasoning with no spatial information
- **CanvasEvent** (spatial, emitted by modality adapter) — describes how a reasoning event maps to canvas cards

```rust
// hive-loop or hive-contracts — what the agent loop emits
pub enum ReasoningEvent {
    StepStarted { step_id: String, description: String },
    ModelCallStarted { model: String, prompt_preview: String },
    ModelCallCompleted { content: String, token_count: u32 },
    ToolCallStarted { tool_id: String, input: Value },
    ToolCallCompleted { tool_id: String, output: Value, is_error: bool },
    BranchEvaluated { condition: String, result: bool },
    PathAbandoned { reason: String },
    Synthesized { sources: Vec<String>, result: String },
    Completed { result: String },
    Failed { error: String },
    TokenDelta { token: String },
}

// hive-canvas — how the spatial modality interprets reasoning events
pub enum CanvasEvent {
    NodeCreated { node: CanvasNode, parent_edge: Option<CanvasEdge> },
    NodeUpdated { node_id: String, patch: NodePatch },
    NodeStatusChanged { node_id: String, status: CardStatus },
    EdgeCreated { edge: CanvasEdge },
    StreamToken { node_id: String, token: String },
}
```

The **DagObserver** consumes `ReasoningEvent`s and produces `CanvasEvent`s. It lives in `hive-canvas`, not `hive-loop`.

### Problem 2: DagObserver takes CanvasStore — observer is coupled to spatial storage

**Leak:** `DagObserver { canvas_store: Arc<dyn CanvasStore> }` means the observer directly writes to the canvas. This makes it impossible to use the DAG construction logic for linear mode.

**Fix:** The DagObserver is a pure function `ReasoningEvent → Vec<CanvasEvent>`. It maintains internal state (card stack, pending ops) but doesn't write to any store. The modality adapter calls the observer, then decides what to do with the resulting `CanvasEvent`s:
- Spatial modality: writes to `CanvasStore`
- Linear modality: extracts text content for the message list, optionally stores DAG metadata for later visualization

### Problem 3: AgentSupervisor takes canvas_store — orchestration coupled to rendering

**Leak:** `AgentSupervisor { canvas_store: Arc<dyn CanvasStore> }` makes multi-agent orchestration depend on spatial storage. The supervisor manages agent lifecycles, message routing, and budget — none of which require canvas knowledge.

**Fix:** The supervisor emits `SupervisorEvent`s (agent spawned, message routed, status changed). The modality layer subscribes to these events and decides how to present them. The supervisor should take an `event_sink: broadcast::Sender<SupervisorEvent>` instead.

### Problem 4: CanvasNode/CanvasEdge in hive-contracts — spatial types in shared layer

**Leak:** The plan says to put `CanvasNode`, `CanvasEdge`, `CardType`, `EdgeType` in `hive-contracts`. This would force every crate that depends on contracts to pull in spatial concepts.

**Fix:** Only modality-agnostic types go in `hive-contracts`:
- `SessionModality` enum
- `ReasoningEvent` enum
- `AgentSpec`, `AgentRole`, `AgentStatus`
- `ConversationModality` trait

Canvas-specific types (`CanvasNode`, `CanvasEdge`, `CardType`, `EdgeType`) stay in `hive-canvas`.

### Problem 5: LayoutHint in AgentEvent — agent suggests rendering layout

**Leak:** `AgentEvent::LayoutSuggestion { arrangement: LayoutHint }` means the agent loop is telling the renderer how to position cards. Layout is a pure rendering concern.

**Fix:** Remove `LayoutSuggestion` from agent output entirely. The layout engine in the spatial modality infers positioning from the DAG structure (parent-child → tree layout, branches → horizontal fan-out, etc.). The agent doesn't need to know.

### Problem 6: ConversationModality trait is too specific

**Leak:** `handle_agent_event(event: AgentEvent)` on the modality trait references `AgentEvent` which (in the original plan) contained `CanvasNode` — creating a circular dependency where the modality trait references spatial types.

**Fix:** The modality trait should consume `ReasoningEvent`s (semantic, modality-agnostic):

```rust
pub trait ConversationModality: Send + Sync {
    fn append_user_message(&self, session_id: &str, content: &str) -> Result<()>;
    fn handle_reasoning_event(&self, session_id: &str, event: &ReasoningEvent) -> Result<()>;
    fn assemble_context(&self, session_id: &str, token_budget: usize) -> Result<Vec<Message>>;
    fn get_snapshot(&self, session_id: &str) -> Result<ConversationSnapshot>;
}
```

### Corrected Layering

```
hive-loop (agent execution)
  │ emits: ReasoningEvent (semantic — no spatial info, no modality coupling)
  │
  ▼
ConversationModality (trait in hive-contracts)
  │
  ├── LinearModality (in hive-api or hive-chat)
  │     Maps ReasoningEvent → ChatMessage in ordered list
  │     Context assembly: recency-based
  │
  └── SpatialModality (in hive-canvas)
        │ Contains: DagObserver, LayoutEngine, SpatialContextAssembler
        │ Maps ReasoningEvent → CanvasEvent → CanvasStore writes
        │ Context assembly: proximity + graph + cluster
        │
        └── CanvasStore (trait + SQLite/R-tree impl, internal to hive-canvas)

hive-agents (multi-agent orchestration)
  │ emits: SupervisorEvent (agent spawned/killed/status changed)
  │ NO dependency on hive-canvas or any modality
  │ The modality layer subscribes to SupervisorEvent and renders appropriately
```

**Key principle:** The agent loop and the agent supervisor emit semantic events about *what happened*. They never reference canvas nodes, positions, or rendering. The modality layer is the ONLY place where reasoning events become visual artifacts.

---

## Problem Statement

HiveMind OS currently supports only a linear chat modality. The spatial-chat and agent-stage concepts describe a richer 2D canvas-based interaction model where agent reasoning becomes visible topology, and multi-agent collaboration is steerable. We need to:

1. **Introduce a modality abstraction** — when creating a new session, the user picks between "Linear Chat" (classic) and "Spatial Chat" (2D canvas). The system must support adding new modalities in the future without restructuring.
2. **Implement Spatial Chat** — a 2D infinite canvas where every message/tool-call/reasoning-step is a card with spatial position and typed edges.
3. **Implement Agent Stage** — a multi-agent collaboration layer where multiple agents are visible characters with roles, status, and data-flow edges. Applicable to BOTH linear and spatial modalities.
4. **Build the backend infrastructure** validated by the 6 PoCs — graph store, DAG observer, multi-agent supervisor, context assembly, canvas rendering, WebSocket sync.

## PoC Results Summary (Validated Assumptions)

| PoC | Key Finding | Implication |
|-----|-------------|-------------|
| Canvas Store | SQLite + R-tree: sub-ms spatial queries at 10k nodes | Use SQLite for graph persistence, in-memory R-tree for spatial index |
| Reasoning DAG | WorkflowEventSink provides enough signal for auto-DAG construction | Build observer pattern, need ~5 more event types |
| Multi-Agent | tokio::mpsc sufficient, no deadlocks with shared backends | No actor framework needed; simple channels + supervisor |
| Canvas Renderer | DOM+SVG+Canvas2D hybrid achieves 60fps at 1000 cards with culling | Hybrid rendering is the right approach |
| Context Assembly | Spatial+Graph F1=1.00 vs Linear F1=0.67 | **Core UX hypothesis validated** — proximity is a meaningful relevance signal |
| WebSocket Sync | Replay buffer sufficient for v1, CRDT not needed initially | Start simple, add CRDT later for multi-user |

---

## Architecture Overview

### Modality Abstraction

The key design decision: **a modality defines how a session stores, renders, and assembles context from its conversation data.** The agent loop, model routing, tool execution, and classification layers remain modality-agnostic.

```
                    ┌─────────────────────┐
                    │   Session           │
                    │   modality: enum    │
                    │   agent_stage: opt  │
                    └────────┬────────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
       ┌────────────┐ ┌────────────┐ ┌────────────┐
       │  Linear    │ │  Spatial   │ │  Future    │
       │  Modality  │ │  Modality  │ │  Modality  │
       └────────────┘ └────────────┘ └────────────┘
       
       Messages as    Messages as    ...
       ordered list   canvas nodes
       
       Context from   Context from
       recency        proximity +
                      graph edges
```

### Agent Stage (Cross-Modality)

Agent Stage is an OVERLAY on any modality. It adds:
- Multiple concurrent agents with roles, models, and tools
- A supervisor managing agent lifecycle
- Inter-agent message routing
- Visual representation of agent topology

In **linear modality**: agents appear as attributed sections in the chat stream, with a stage panel showing topology.
In **spatial modality**: agents are visible characters on the canvas, with animated data-flow edges.

---

## Phase Breakdown

### Phase S0: Modality Abstraction & Session Model (Foundation)

**Goal:** Introduce the modality concept into the session model so all downstream work builds on a stable abstraction.

#### S0.1: Core Types in `hive-contracts`

Add modality types to the shared contracts:

```rust
/// The interaction modality for a session
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionModality {
    Linear,
    Spatial,
}

/// Extended session config passed at creation time
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionCreateRequest {
    pub title: Option<String>,
    pub modality: SessionModality,        // NEW
    pub preferred_model: Option<String>,
    pub system_prompt: Option<String>,
}
```

Update `ChatSessionSnapshot` to include `modality: SessionModality`.

#### S0.2: Session Creation API

- Modify `POST /api/v1/chat/sessions` to accept `SessionCreateRequest` with modality field
- Default to `Linear` if not specified (backward compat)
- Store modality in session record
- Frontend "New Chat" flow presents modality picker:
  - 💬 **Classic Chat** — Linear conversation
  - 🧭 **Spatial Canvas** — 2D reasoning canvas

#### S0.3: Modality Trait / Strategy Pattern

Define a backend trait that encapsulates modality-specific behavior:

```rust
/// Modality determines how conversation data is stored and context is assembled
pub trait ConversationModality: Send + Sync {
    /// Store a new user message in this session's conversation
    fn append_user_message(&self, session_id: &str, content: &str) -> Result<()>;
    
    /// Handle a semantic reasoning event from the agent loop
    fn handle_reasoning_event(&self, session_id: &str, event: &ReasoningEvent) -> Result<()>;
    
    /// Assemble context for the next model call
    fn assemble_context(&self, session_id: &str, token_budget: usize) -> Result<Vec<Message>>;
    
    /// Get the conversation state for frontend rendering
    fn get_snapshot(&self, session_id: &str) -> Result<ConversationSnapshot>;
}
```

Two implementations:
- `LinearModality` — wraps the current `Vec<ChatMessage>` approach
- `SpatialModality` — uses the canvas graph store

This trait is injected into `ChatService` and selected based on the session's modality at creation time.

---

### Phase S1: Canvas Graph Store (`hive-canvas` crate)

**Goal:** Persistent directed property graph with spatial indexing. Foundation for spatial chat.

#### S1.1: Data Model

Types in `hive-canvas/src/types.rs`:

```rust
pub struct CanvasNode {
    pub id: String,
    pub canvas_id: String,           // session ID
    pub card_type: CardType,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub content: serde_json::Value,  // type-specific payload
    pub status: CardStatus,          // Active, DeadEnd, Archived
    pub created_by: String,          // user ID or agent ID
    pub created_at: i64,
}

pub enum CardType {
    Prompt, Response, Artifact, Reference, Cluster,
    Decomposition, ToolCall, DecisionPoint, Synthesis, DeadEnd,
}

pub enum CardStatus { Active, DeadEnd, Archived }

pub struct CanvasEdge {
    pub id: String,
    pub canvas_id: String,
    pub source_id: String,
    pub target_id: String,
    pub edge_type: EdgeType,
    pub metadata: serde_json::Value,
    pub created_at: i64,
}

pub enum EdgeType {
    ReplyTo, References, Contradicts, Evolves,
    DecomposesTo, ToolIO, Synthesizes,
    // Agent Stage edges:
    Delegation, ContextShare, ArtifactPass, FeedbackLoop, BlockedBy,
}
```

#### S1.2: Store Trait & SQLite Implementation

```rust
pub trait CanvasStore: Send + Sync {
    // CRUD
    fn insert_node(&self, node: &CanvasNode) -> Result<()>;
    fn insert_edge(&self, edge: &CanvasEdge) -> Result<()>;
    fn update_node_position(&self, node_id: &str, x: f64, y: f64) -> Result<()>;
    fn update_node_content(&self, node_id: &str, content: Value) -> Result<()>;
    fn update_node_status(&self, node_id: &str, status: CardStatus) -> Result<()>;
    fn delete_node(&self, node_id: &str) -> Result<()>;  // cascades edges
    fn get_node(&self, node_id: &str) -> Result<Option<CanvasNode>>;
    fn get_edges_from(&self, node_id: &str) -> Result<Vec<CanvasEdge>>;
    fn get_edges_to(&self, node_id: &str) -> Result<Vec<CanvasEdge>>;
    
    // Spatial queries
    fn query_viewport(&self, canvas_id: &str, min_x: f64, min_y: f64, max_x: f64, max_y: f64) -> Result<Vec<CanvasNode>>;
    fn query_radius(&self, canvas_id: &str, cx: f64, cy: f64, radius: f64) -> Result<Vec<CanvasNode>>;
    fn query_nearest(&self, canvas_id: &str, cx: f64, cy: f64, k: usize) -> Result<Vec<CanvasNode>>;
    
    // Graph traversal
    fn bfs(&self, start_id: &str, max_depth: usize) -> Result<Vec<CanvasNode>>;
    fn connected_component(&self, node_id: &str) -> Result<Vec<CanvasNode>>;
    
    // Bulk
    fn get_all_nodes(&self, canvas_id: &str) -> Result<Vec<CanvasNode>>;
    fn get_all_edges(&self, canvas_id: &str) -> Result<Vec<CanvasEdge>>;
}
```

Implementation: SQLite tables + in-memory R-tree (using `rstar` crate). R-tree rebuilt on startup from persisted positions. Updates go to both SQLite and R-tree.

#### S1.3: Tests

- Unit tests for CRUD, spatial queries (viewport, radius, nearest), graph traversal (BFS, connected components)
- Benchmark: 10k nodes, sub-ms spatial queries (validated by PoC)

---

### Phase S2: Reasoning Events & DAG Observer

**Goal:** The agent loop emits semantic `ReasoningEvent`s (modality-agnostic). The spatial modality contains a `DagObserver` that maps those events to canvas cards.

#### S2.1: ReasoningEvent Protocol (in `hive-contracts` or `hive-loop`)

These are semantic events about what the agent *did*, with no spatial or rendering information:

```rust
pub enum ReasoningEvent {
    StepStarted { step_id: String, description: String },
    ModelCallStarted { model: String, prompt_preview: String },
    ModelCallCompleted { content: String, token_count: u32 },
    ToolCallStarted { tool_id: String, input: Value },
    ToolCallCompleted { tool_id: String, output: Value, is_error: bool },
    BranchEvaluated { condition: String, result: bool },
    PathAbandoned { reason: String },
    Synthesized { sources: Vec<String>, result: String },
    Completed { result: String },
    Failed { error: String },
    TokenDelta { token: String },
}
```

**Key:** This replaces the current `LoopEvent` enum (or extends it). Every strategy (ReAct, Sequential, PlanThenExecute) emits these events. The modality layer consumes them.

#### S2.2: DagObserver (in `hive-canvas`, NOT `hive-loop`)

A **pure transformer** with no storage dependencies:

```rust
pub struct DagObserver {
    card_stack: Vec<String>,        // nesting context for parent-child
    pending_model: Option<PendingCall>,
    pending_tools: HashMap<String, PendingCall>,
    layout_engine: TreeLayoutEngine,
    id_generator: IdGenerator,
}

impl DagObserver {
    /// Transforms a semantic reasoning event into canvas-specific events
    pub fn observe(&mut self, event: &ReasoningEvent) -> Vec<CanvasEvent> {
        // Maps reasoning events to CanvasNode/CanvasEdge creation
        // Maintains nesting via card_stack
        // Auto-positions using layout_engine
        // Returns CanvasEvents that the SpatialModality writes to store
    }
}
```

The observer lives in `hive-canvas` because it produces canvas-specific types. It does NOT take a `CanvasStore` — it's a stateless event mapper. The `SpatialModality` calls `observer.observe(event)` and writes the resulting `CanvasEvent`s to the store itself.

#### S2.3: Layout Engine (in `hive-canvas`)

Simple hierarchical tree layout for v1:
- Children positioned below parent
- Siblings spread horizontally with spacing
- Dead ends visually offset
- Tool call → result pairs grouped vertically

The layout engine is called by the DagObserver to assign (x, y) coordinates to newly created nodes. It has no dependency on the store — it maintains an in-memory tree structure of node positions.

Force-directed layout deferred to v2.

#### S2.4: Modality Consumption — How Each Modality Handles ReasoningEvents

**LinearModality** (in `hive-api`):
- `ModelCallCompleted` → append assistant `ChatMessage` to session
- `TokenDelta` → forward as streaming token to frontend
- `ToolCallStarted/Completed` → embed in assistant message or side panel
- All other events → optional metadata, ignored for rendering

**SpatialModality** (in `hive-canvas`):
- All events → `DagObserver::observe()` → `Vec<CanvasEvent>` → write to `CanvasStore`
- `TokenDelta` → stream into the active card's content
- Layout computed automatically by the observer's layout engine

**Important:** Both modalities receive the SAME `ReasoningEvent` stream. The agent loop doesn't know which modality is active.

---

### Phase S3: Context Assembly

**Goal:** When a user creates a prompt in spatial mode, assemble LLM context from proximity + graph + cluster signals.

#### S3.1: Context Assembler

```rust
pub struct SpatialContextAssembler {
    canvas_store: Arc<dyn CanvasStore>,
    token_counter: Arc<dyn TokenCounter>,
}

impl SpatialContextAssembler {
    pub fn assemble(
        &self,
        prompt_card: &CanvasNode,
        canvas_id: &str,
        token_budget: usize,
    ) -> Result<Vec<ContextCard>> {
        let mut layers = Vec::new();
        
        // Layer 1: Direct edges (required)
        // Layer 2: Spatial proximity (high priority)
        // Layer 3: Same cluster (medium priority)
        // Layer 4: Extended graph BFS depth=3 (low priority)
        
        // Deduplicate, prioritize, truncate to budget
    }
}
```

#### S3.2: Token Counter

Use `tiktoken-rs` for OpenAI-compatible models, approximate for others (chars/4).

```rust
pub trait TokenCounter: Send + Sync {
    fn count(&self, text: &str, model: &str) -> usize;
}
```

#### S3.3: Integration with Modality Trait

The `SpatialModality::assemble_context()` implementation delegates to `SpatialContextAssembler`. The `LinearModality::assemble_context()` uses the existing recency-based approach.

---

### Phase S4: Multi-Agent Orchestration (`hive-agents` crate)

**Goal:** Multiple agents running concurrently with a supervisor, inter-agent messaging, and topology patterns. This is the Agent Stage backend.

#### S4.1: Agent Types

```rust
pub struct AgentSpec {
    pub id: String,
    pub name: String,
    pub role: AgentRole,
    pub model: Option<String>,         // preferred model
    pub system_prompt: String,
    pub allowed_tools: Vec<String>,
    pub avatar: Option<String>,        // icon/emoji
    pub color: Option<String>,         // for UI
}

pub enum AgentRole {
    Planner, Researcher, Coder, Reviewer, Writer, Analyst,
    Custom(String),
}

pub enum AgentStatus {
    Spawning, Active, Waiting, Paused, Blocked, Done, Error,
}
```

#### S4.2: Agent Supervisor

```rust
pub struct AgentSupervisor {
    agents: DashMap<String, AgentHandle>,
    event_tx: broadcast::Sender<SupervisorEvent>,
    model_router: Arc<ModelRouter>,
    tool_registry: Arc<ToolRegistry>,
    budget: AtomicBudget,
}
```

**Note:** The supervisor does NOT take a `CanvasStore` or any modality-specific dependency. It emits `SupervisorEvent`s (agent spawned, message routed, status changed, completed). The modality layer subscribes to these events:
- **Linear modality:** renders agent output as attributed messages (`[🔍 Researcher]: ...`)
- **Spatial modality:** creates agent cards on canvas, animates data-flow edges

```rust
pub enum SupervisorEvent {
    AgentSpawned { agent_id: String, spec: AgentSpec },
    AgentStatusChanged { agent_id: String, status: AgentStatus },
    MessageRouted { from: String, to: String, msg_type: String },
    AgentOutput { agent_id: String, event: ReasoningEvent },
    AgentCompleted { agent_id: String, result: String },
    AllComplete { total_tokens: u64, total_cost: f64 },
}
```

The key insight: `AgentOutput` wraps a `ReasoningEvent` — each agent's reasoning stream uses the same semantic event type. The modality layer can then route each agent's events through its own DagObserver (spatial) or message formatter (linear).

```rust
impl AgentSupervisor {
    pub async fn spawn_agent(&self, spec: AgentSpec) -> Result<String>;
    pub async fn send_to_agent(&self, agent_id: &str, msg: AgentMessage) -> Result<()>;
    pub async fn broadcast(&self, msg: AgentMessage) -> Result<()>;
    pub async fn whisper(&self, agent_id: &str, msg: AgentMessage) -> Result<()>;
    pub async fn pause(&self, agent_id: &str) -> Result<()>;
    pub async fn resume(&self, agent_id: &str) -> Result<()>;
    pub async fn kill(&self, agent_id: &str) -> Result<()>;
    pub async fn recast(&self, agent_id: &str, new_spec: AgentSpec) -> Result<()>;
    pub fn get_topology(&self) -> AgentTopology;
}
```

#### S4.3: Topology Patterns

Topologies expressed as YAML workflow definitions (extending the existing hive-loop DSL):

```yaml
topology: fan-out
director_prompt: "Build a REST API for user management"
agents:
  - id: planner
    role: planner
    model: gpt-4o
  - id: researcher
    role: researcher
    model: claude-sonnet
  - id: coder
    role: coder
    model: gpt-4o
flow:
  - from: planner
    to: [researcher, coder]
    type: fan-out
  - from: [researcher, coder]
    to: synthesizer
    type: fan-in
```

#### S4.4: Inter-Agent Messaging

Each agent gets a `tokio::mpsc` inbox. The supervisor routes messages based on topology edges. Message types:

```rust
pub enum AgentMessage {
    Task { content: String, context: Vec<ContextCard> },
    Result { content: String, artifacts: Vec<String> },
    Feedback { content: String, from: String },
    Broadcast { content: String, from: String },
    Directive { content: String },  // from user
    Control(ControlSignal),
}
```

#### S4.5: Agent Stage Integration with Both Modalities

**Linear modality + Agent Stage:**
- Chat stream shows attributed messages: `[🔍 Researcher]: Found 3 relevant files...`
- Collapsible stage panel on the side shows topology diagram (simple SVG)
- Director's console at bottom with broadcast/whisper/pause controls

**Spatial modality + Agent Stage:**
- Each agent is a persistent card on the canvas with avatar, status, progress
- Data-flow edges between agents are animated
- Agent work products are cards positioned near the agent that created them
- Director's console is part of the canvas HUD

---

### Phase S5: Frontend — Spatial Canvas (v1)

**Goal:** 2D infinite canvas with pan/zoom, card rendering, edge rendering, and viewport culling.

#### S5.1: Canvas Engine

SolidJS-based hybrid renderer (validated by PoC):

| Layer | Technology | Content |
|-------|-----------|---------|
| Background | Canvas2D | Grid lines, region shading |
| Edges | SVG overlay | Typed edges with bezier curves |
| Cards | DOM elements | Rich content, interactive, accessible |
| HUD | Fixed DOM | Toolbar, zoom controls, global prompt bar, minimap |

#### S5.2: Viewport & Coordinate System

```typescript
interface CanvasViewport {
    centerX: number;
    centerY: number;
    zoom: number;        // 0.05 (galaxy) → 2.0 (surface)
    screenWidth: number;
    screenHeight: number;
}

// Coordinate transforms
canvasToScreen(cx, cy, viewport) → [sx, sy]
screenToCanvas(sx, sy, viewport) → [cx, cy]
```

Culling: only render cards whose bounding box intersects viewport + buffer. Client-side R-tree (rbush.js) for O(log n) queries.

#### S5.3: Card Components

One SolidJS component per card type:
- `PromptCard` — user input, editable
- `ResponseCard` — agent reply, streaming support
- `ArtifactCard` — code blocks, images, files
- `ToolCallCard` — tool name, input/output, status
- `DecompositionCard` — branching indicator
- `DecisionPointCard` — diamond shape, chosen + rejected options
- `SynthesisCard` — merge indicator
- `DeadEndCard` — grayed out, strikethrough
- `AgentCard` — avatar, role, status, progress (for Agent Stage)

All cards share: drag-to-move, click-to-expand, contextual menu, edge attachment points.

#### S5.4: Input Model

Three ways to prompt:
1. **Global prompt bar** — bottom of screen, creates card at smart default position
2. **Contextual prompt** — click empty canvas space, inline text field appears at that position
3. **Card-attached prompt** — reply button on any card, new prompt anchors to that card

#### S5.5: Edge Rendering

SVG paths with quadratic bezier curves. Visual styles per edge type:
- Reply-to: solid gray arrow
- References: dashed blue line
- Contradicts: red double-headed
- Tool-IO: colored by tool type
- Delegation/Artifact-pass: animated particles

#### S5.6: Zoom Levels (LOD)

| Level | Zoom Range | Rendering |
|-------|-----------|-----------|
| Galaxy | 0.05–0.15 | Clusters as colored blobs, labels only |
| Constellation | 0.15–0.4 | Card outlines with type icons, edge lines |
| System | 0.4–1.2 | Full card content, readable text |
| Surface | 1.2–2.0 | Single card focus, full detail |

---

### Phase S6: WebSocket Real-Time Sync

**Goal:** Bidirectional sync between backend agent events and frontend canvas state.

#### S6.1: WebSocket Transport

Add WebSocket upgrade route in `hive-api`:

```
GET /api/v1/canvas/{session_id}/ws?last_sequence=N
```

Server maintains per-session event log with monotonic sequence numbers. On connect:
1. Send `Welcome { client_id, current_sequence }`
2. If `last_sequence` provided, replay missed events
3. Stream live `AgentEvent`s as they occur

#### S6.2: Client → Server Messages

```rust
enum ClientMessage {
    PositionUpdate { card_id: String, x: f64, y: f64 },
    CardCreate { card: CanvasNode },
    CardDelete { card_id: String },
    PromptSubmit { card_id: String, content: String },
    CursorMove { x: f64, y: f64 },
}
```

#### S6.3: Reconnection & Replay

Client tracks `last_sequence`. On disconnect/reconnect, sends `last_sequence` to get missed events. No full CRDT needed for v1 (single-user desktop app).

---

### Phase S7: Telemetry & Cost Tracking

**Goal:** Per-agent token/cost tracking, budget fences, and dashboard UI.

#### S7.1: Token Accumulator

```rust
pub struct TokenAccumulator {
    per_agent: DashMap<String, TokenUsage>,
    total: AtomicTokenUsage,
    budget_limit: Option<f64>,  // dollars
}

pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub estimated_cost: f64,
    pub model_calls: u32,
    pub tool_calls: u32,
}
```

#### S7.2: Budget Fence

When accumulated cost reaches 80% of limit → emit warning event.
When 100% reached → pause all agents, emit `BudgetExceeded`, wait for user decision.

#### S7.3: Telemetry Dashboard UI

Collapsible bottom panel (both modalities):
- Active agents count, total tokens, total cost
- Per-agent breakdown bars
- Cost velocity projection
- Budget controls

---

## Dependency Graph

```
S0 (Modality Abstraction)
 ├── S1 (Canvas Graph Store)
 │    ├── S2 (Agent Events & DAG Observer)
 │    │    └── S5 (Frontend Canvas v1)
 │    │         └── S6 (WebSocket Sync)
 │    └── S3 (Context Assembly)
 └── S4 (Multi-Agent Orchestration)
      └── S7 (Telemetry & Cost Tracking)
```

- S0 is the foundation — must come first
- S1 and S4 can proceed in parallel after S0
- S2 depends on S1 (needs canvas store)
- S3 depends on S1 (needs spatial queries)
- S5 depends on S1 + S2 (needs data model + events)
- S6 depends on S5 (needs frontend canvas)
- S7 depends on S4 (needs agent tracking)
- Agent Stage is cross-cutting: S4 defines backend, S5 defines frontend

## Crate Impact

| Crate | Changes | What does NOT belong here |
|-------|---------|--------------------------|
| `hive-contracts` | `SessionModality`, `ReasoningEvent`, `AgentSpec`, `AgentRole`, `AgentStatus`, `ConversationModality` trait, `SupervisorEvent` | No canvas types (CanvasNode, CardType, etc.) |
| `hive-canvas` (NEW) | `CanvasNode`, `CanvasEdge`, `CardType`, `EdgeType`, `CanvasStore` trait + SQLite/R-tree impl, `DagObserver`, `TreeLayoutEngine`, `SpatialModality`, `SpatialContextAssembler` | Observer does NOT take CanvasStore — it's a pure event transformer |
| `hive-agents` (NEW) | `AgentSupervisor`, `AgentRunner`, topologies, inter-agent messaging, budget tracking | No canvas dependency, no modality dependency |
| `hive-loop` | Emit `ReasoningEvent`s from strategies, extend `WorkflowEventSink` | No canvas types, no modality types — just semantic events |
| `hive-api` | Session creation with modality, `LinearModality` impl, WebSocket routes, supervisor API, modality routing in `ChatService` | No canvas types in API layer |
| `hive-model` | No changes (modality-agnostic) | — |
| `hive-tools` | No changes (shared across agents) | — |
| `hivemind-desktop` (frontend) | Modality picker, canvas engine, card components, edge renderer, HUD, agent stage panel, linear chat (unchanged) | — |

## Implementation Order (Recommended)

1. **S0** — Modality abstraction + session model changes
2. **S1** — Canvas graph store crate
3. **S2 + S3** — Agent events + context assembly (parallel)
4. **S4** — Multi-agent orchestration
5. **S5** — Frontend canvas v1 (largest phase — cards + interactions, then polish)
6. **S6** — WebSocket sync
7. **S7** — Telemetry

## Key Design Decisions

1. **Modality is a session-level choice**, not a global setting. Users can have linear and spatial sessions simultaneously.
2. **Agent Stage is orthogonal to modality.** A linear chat can have multiple agents. A spatial chat can have a single agent.
3. **The canvas graph store is separate from the knowledge graph.** The knowledge graph (`hive-knowledge`) is for long-term memory across sessions. The canvas graph (`hive-canvas`) is the conversation state within a session.
4. **No CRDT for v1.** The desktop app is single-user. Simple WebSocket event streaming with replay buffer is sufficient. CRDT can be added later for multi-user/team scenarios.
5. **DAG observer is automatic, not manual.** Agents don't need to explicitly emit card events — the observer watches workflow events and infers the card topology. This means existing strategies (ReAct, Sequential) get spatial visualization "for free."
6. **Context assembly is pluggable.** The `ConversationModality` trait's `assemble_context()` method allows each modality to define its own context strategy. Spatial uses proximity+graph. Linear uses recency. Future modalities can define their own.
7. **Agent roster is config-driven.** Pre-defined agent types (Planner, Researcher, Coder, etc.) with customizable models/prompts. Users can create custom agents saved to config.
8. **Semantic events, not canvas events, cross crate boundaries.** The agent loop and supervisor emit `ReasoningEvent` and `SupervisorEvent` (semantic, position-free). Only the spatial modality adapter in `hive-canvas` maps these to spatial `CanvasEvent`s. No crate outside `hive-canvas` ever references `CanvasNode`, `CanvasEdge`, or coordinates.
9. **The DagObserver is a pure transformer, not a writer.** It takes `ReasoningEvent`s and returns `CanvasEvent`s. It doesn't hold a `CanvasStore` reference. The `SpatialModality` is responsible for writing to the store — this keeps the observer testable and reusable.
10. **The AgentSupervisor has no rendering or storage dependencies.** It manages agent lifecycles and inter-agent messaging via channels. It emits `SupervisorEvent`s that the active modality consumes and renders however it chooses.

## Open Questions (To Resolve During Implementation)

- **Card persistence granularity:** Should every streaming token create a DB write, or batch at card completion?
  → Recommendation: Optimistic in-memory cards, persist on completion or every N seconds.
  
- **Agent autonomy slider:** How much auto-coordination vs. user-directed?
  → Recommendation: Start fully user-directed (manual topology), add auto-coordination later.
  
- **Mobile/responsive:** How does spatial chat work on small screens?
  → Recommendation: Defer. Desktop-first. Add outline/rail fallback later.

- **Cross-session canvas linking:** Can cards reference cards in other sessions?
  → Recommendation: Not in v1. Each canvas is session-scoped. Cross-session references via knowledge graph.
