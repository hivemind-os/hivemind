# Proof-of-Concept Proposals

Targeted PoCs to validate the riskiest assumptions behind the **Spatial Chat** and **Multi-Agent Stage** concepts before committing to full infrastructure builds.

Each PoC is self-contained, buildable in the `poc/` directory, and designed to answer one or two specific questions.

---

## PoC 1 — Canvas Graph Store + Spatial Queries

**Assumption being tested:** SQLite with JSON columns and an in-memory R-tree (`rstar` crate) can serve as a performant graph store with spatial queries for up to 10k nodes.

**What to build:**
- A Rust library crate (`poc/canvas-store`) with:
  - `CanvasNode` and `CanvasEdge` types matching the spatial-chat data model
  - SQLite-backed CRUD (rusqlite, JSON columns for metadata/content)
  - In-memory R-tree (`rstar`) synced from DB on startup, updated on mutations
  - Spatial queries: viewport culling (bounding-box), radius proximity, k-nearest neighbors
  - Graph traversal: BFS/DFS to depth N, connected components
- Benchmarks:
  - Insert 10k nodes, measure query latency for viewport cull (should be < 1ms)
  - Measure spatial-query-then-graph-traverse patterns (assemble context for a card)
  - Concurrent read/write under Tokio (multiple agents writing cards while UI reads)

**Key questions answered:**
1. Can SQLite + R-tree handle the access patterns needed for context assembly at interactive speeds?
2. What's the memory overhead of an in-memory R-tree for 10k–100k nodes?
3. Is the R-tree cache sync with SQLite reliable under concurrent writes?

**Risks mitigated:** If SQLite is too slow for spatial queries, we'd need a dedicated spatial DB (e.g., SpatialLite extension, or an embedded graph DB). Better to know now.

**Estimated scope:** ~500 lines of Rust, 1-2 days.

---

## PoC 2 — Reasoning DAG Observer

**Assumption being tested:** The existing `WorkflowEventSink` trait in `hive-loop` provides enough signal to automatically construct a reasoning topology (card DAG) from a workflow execution, without modifying the workflow engine itself.

**What to build:**
- A new `WorkflowEventSink` implementation (`poc/reasoning-dag`) that:
  - Observes workflow events (model calls, tool calls, branches, loops, completions)
  - Constructs a DAG of typed cards (decomposition, tool-call, decision-point, synthesis, dead-end)
  - Auto-assigns spatial positions using a simple tree layout algorithm (children fan out below parent)
  - Outputs the DAG as a JSON file for visualization
- A simple HTML viewer (single file, no build step) that renders the DAG as an interactive SVG tree
- Wire it into an existing workflow (e.g., the built-in `react.yaml`) and run against a real prompt

**Key questions answered:**
1. Does the current `WorkflowEventSink` emit enough event types to distinguish decomposition vs. tool-call vs. synthesis vs. dead-end?
2. What additional event types would need to be added to `hive-loop`?
3. Does the auto-layout produce legible topologies, or do we need user-driven positioning from the start?
4. Can the DAG observer be purely additive (no changes to engine internals)?

**Risks mitigated:** If the event sink doesn't provide enough signal, we'd need to redesign the workflow engine's event model before building the canvas — a much larger change. This PoC reveals the gap cheaply.

**Estimated scope:** ~400 lines Rust + ~200 lines HTML/JS, 1-2 days.

---

## PoC 3 — Multi-Agent Supervisor

**Assumption being tested:** Multiple `WorkflowEngine` instances can run concurrently under a supervisor, with inter-agent messaging via `tokio::mpsc` channels, without deadlocks or resource contention on shared model/tool backends.

**What to build:**
- A Rust binary (`poc/multi-agent`) with:
  - `AgentSpec` struct: id, name, role, system prompt, model binding, tool list
  - `AgentSupervisor` that spawns N agents as concurrent Tokio tasks
  - Each agent runs its own `WorkflowEngine` (or a simplified mock)
  - Inter-agent messaging: `tokio::mpsc` channels with a routing table (agent A can send to agent B)
  - Delegation: agent A emits a `Delegate { target_agent, sub_task }` action → supervisor routes it
  - Director console: stdin-based commands to broadcast, whisper, pause, resume, kill agents
  - Per-agent token/cost accumulator (mock model backend that simulates token usage)
- Test scenarios:
  - **Pipeline:** A → B → C (sequential delegation)
  - **Fan-out/Fan-in:** A spawns B, C, D in parallel; waits for all results; synthesizes
  - **Feedback loop:** A ↔ B iterate until quality condition met (max 3 rounds)

**Key questions answered:**
1. Can the existing `ModelBackend` / `ToolBackend` trait objects be safely shared across concurrent agent tasks?
2. What happens when two agents try to use the same local model simultaneously (contention on `RuntimeManager` mutex)?
3. Is `tokio::mpsc` the right primitive, or do we need something more structured (e.g., an actor framework)?
4. How does the supervisor handle cascade failures (agent B dies → agent A is waiting on it)?

**Risks mitigated:** Multi-agent orchestration is the single largest new system. If shared backends cause deadlocks, or if the messaging model is wrong, the whole Agent Stage design needs rethinking. This PoC stress-tests the concurrency model.

**Estimated scope:** ~800 lines Rust, 2-3 days.

---

## PoC 4 — Hybrid Canvas Renderer

**Assumption being tested:** A hybrid DOM-cards + SVG-edges + Canvas2D-background rendering approach in SolidJS can maintain 60fps with 200+ visible cards and animated edge particles, with viewport culling via a client-side R-tree.

**What to build:**
- A standalone SolidJS app (`poc/canvas-renderer`) with:
  - Infinite canvas with pan (scroll/drag) and zoom (pinch/scroll)
  - Viewport state: center, zoom level, computed bounding box
  - Client-side R-tree (JS `rbush` library) for viewport culling
  - DOM card rendering: cards as `<div>` with `transform: translate(x,y) scale(z)`, only mounted when in viewport
  - SVG edge layer: bezier curves between cards, with animated dash-offset for "data flowing" effect
  - Canvas2D background: subtle grid, region highlights
  - Card types: prompt (blue), response (green), tool-call (orange), dead-end (gray), synthesis (purple)
  - Interactions: drag to reposition cards, click to select, double-click to expand
  - Generate 500 random cards in a tree structure for stress testing
- Performance measurements:
  - FPS during pan/zoom with 200, 500, 1000 cards in the scene
  - DOM node count in viewport at each zoom level
  - Memory usage growth over time (check for leaks during pan)

**Key questions answered:**
1. Does SolidJS's fine-grained reactivity handle dynamic mount/unmount of 200+ cards without frame drops?
2. Is `rbush` fast enough for real-time viewport culling on every frame?
3. Does the hybrid DOM+SVG+Canvas2D approach produce visual artifacts at boundaries or during zoom transitions?
4. What's the practical upper limit on visible cards before we need virtualization or WebGL cards?

**Risks mitigated:** The frontend canvas is the biggest UI investment. If the hybrid approach doesn't perform, we'd need to evaluate full WebGL rendering (pixi.js/konva), which changes the entire frontend architecture. This PoC answers the performance question before we commit.

**Estimated scope:** ~600 lines TSX/CSS, 2 days.

---

## PoC 5 — Context Assembly from Spatial Proximity

**Assumption being tested:** Spatial proximity is a useful signal for context assembly — cards near a prompt are more relevant than distant cards — and the resulting context produces better LLM responses than naive "last N messages" assembly.

**What to build:**
- A Rust CLI tool (`poc/context-assembly`) that:
  - Loads a canvas graph from a JSON file (a pre-built scenario with ~50 cards across 3-4 topic clusters)
  - Implements the 4-layer context assembly algorithm:
    1. Direct edges (always included)
    2. Spatial proximity (R-tree radius query)
    3. Same-cluster siblings
    4. Graph-connected within N hops
  - Token budgeting: approximate token count (chars/4), truncate layers from lowest priority up
  - Outputs the assembled context as a formatted prompt
  - Compares three assembly strategies on the same scenario:
    - **Linear:** last 10 messages (current approach)
    - **Graph-only:** follow edges, ignore positions
    - **Spatial+Graph:** the full 4-layer algorithm
- A test scenario:
  - User has two clusters: "auth system" (top-left) and "data pipeline" (bottom-right)
  - New prompt card placed near the auth cluster: "How should we handle token refresh?"
  - Measure: does spatial assembly include auth cards and exclude pipeline cards? Does linear assembly include irrelevant pipeline discussion?

**Key questions answered:**
1. Does spatial proximity actually correlate with semantic relevance, or is it just noise?
2. How sensitive is the output to the proximity radius parameter?
3. What's the right balance between the 4 context layers for typical scenarios?
4. Is approximate token counting (chars/4) accurate enough, or do we need a real tokenizer?

**Risks mitigated:** The entire value proposition of spatial chat rests on "proximity = relevance." If this doesn't hold — if users don't naturally cluster related cards, or if the radius-based selection is too coarse — the concept needs redesign. This is the cheapest way to test the core UX hypothesis.

**Estimated scope:** ~400 lines Rust + ~100 lines JSON scenarios, 1-2 days.

---

## PoC 6 — WebSocket Event Stream

**Assumption being tested:** Axum WebSocket upgrades can replace the current SSE/broadcast channel for real-time streaming, supporting bidirectional communication (server→client card events + client→server position updates) without breaking existing streaming UX.

**What to build:**
- A minimal Axum server (`poc/ws-canvas-sync`) with:
  - WebSocket upgrade endpoint at `/ws/canvas/{session_id}`
  - Server-side: simulates an agent producing cards at 1-2 per second, sends `CardCreated`/`CardUpdated` JSON events
  - Client-side: SolidJS page that connects via WebSocket, renders cards as they arrive, sends position updates when cards are dragged
  - Presence: each connected client sends cursor position at ~10Hz, server broadcasts to all others
  - Reconnection: client auto-reconnects on disconnect, server replays missed events from a short buffer
- Test scenarios:
  - 2 browser tabs connected to the same session, verify both see card events and each other's cursors
  - Kill and restart the server, verify clients reconnect and resync
  - Measure latency: time from server event emit to client render

**Key questions answered:**
1. Can we run WebSocket alongside existing HTTP routes in the same Axum server?
2. What's the message throughput before backpressure becomes an issue?
3. Is a simple event replay buffer sufficient for reconnection, or do we need full CRDT from the start?
4. How does SolidJS handle high-frequency reactive updates from WebSocket (cursor positions at 10Hz)?

**Risks mitigated:** Real-time sync is identified as the "single biggest infrastructure addition" in the infrastructure doc. This PoC validates the simplest viable approach (WebSocket + event replay) before considering CRDT complexity.

**Estimated scope:** ~500 lines Rust + ~300 lines TSX, 2 days.

---

## Priority & Sequencing

The PoCs are ordered by **risk reduction per unit effort** — do the ones that test the most dangerous assumptions first.

| Priority | PoC | Core Risk Tested | Depends On |
|----------|-----|------------------|------------|
| **1st** | PoC 5 — Context Assembly | Does proximity = relevance? (UX hypothesis) | None |
| **2nd** | PoC 1 — Canvas Graph Store | Can SQLite + R-tree handle the data model? | None |
| **3rd** | PoC 2 — Reasoning DAG Observer | Is WorkflowEventSink sufficient for card generation? | None |
| **4th** | PoC 4 — Hybrid Canvas Renderer | Does DOM+SVG+Canvas2D perform at scale? | None |
| **5th** | PoC 3 — Multi-Agent Supervisor | Does concurrent agent execution work? | None |
| **6th** | PoC 6 — WebSocket Event Stream | Can we do real-time sync simply? | PoC 1 (data model) |

PoCs 1-5 are independent and can be built in parallel. PoC 6 benefits from having the data model from PoC 1 defined first.

**Recommendation:** Start with PoC 5 (context assembly) because it tests the **core UX hypothesis** of spatial chat — if proximity doesn't correlate with relevance, the entire concept needs rethinking, and the other PoCs become moot. PoC 5 is also the smallest and cheapest to build.

---

## What Each PoC Tells Us About the Full Build

| If PoC succeeds... | Confidence gained |
|---------------------|-------------------|
| PoC 1 passes benchmarks | Proceed with SQLite-based `hive-canvas` crate; no need for external graph DB |
| PoC 2 produces legible DAGs | Current `hive-loop` event model is sufficient; card emitter can be built as an observer |
| PoC 3 handles concurrency | `tokio::mpsc` + shared backends is viable; no need for actor framework |
| PoC 4 holds 60fps | Hybrid rendering approach works; no need for full WebGL rewrite |
| PoC 5 shows relevance correlation | Spatial context assembly is validated; proceed with full UX |
| PoC 6 reconnects cleanly | WebSocket + replay buffer is sufficient for v1; defer CRDT to v2 |

| If PoC fails... | Pivot required |
|------------------|---------------|
| PoC 1 too slow | Evaluate SpatiaLite extension or embedded graph DB (surrealdb, etc.) |
| PoC 2 insufficient events | Redesign `WorkflowEventSink` with richer event taxonomy before building canvas |
| PoC 3 deadlocks | Need per-agent model instances or actor-based isolation (e.g., `actix`) |
| PoC 4 drops frames | Move to full WebGL card rendering (pixi.js) or canvas virtualization |
| PoC 5 no correlation | Reconsider spatial chat — maybe graph-only context (no positions) is sufficient |
| PoC 6 data loss on reconnect | Full CRDT (yrs/automerge) needed from v1, not just event replay |
