# Spatial Chat — 2D Canvas for Agentic Conversations

## The Core Insight

Linear chat is a **lossy format**. Real thinking is non-linear — you explore tangents, circle back, connect disparate ideas. A linear thread forces branching thoughts into a single stream, losing the structure of how ideas relate. Spatial chat makes that structure **visible and manipulable**.

## Mental Model

Think of it as a mashup of:
- **Miro/FigJam** (infinite canvas, spatial arrangement)
- **Obsidian graph view** (nodes with semantic connections)
- **Chat** (conversational interaction with an agent)

The canvas *is* the conversation. Every message — user or agent — becomes a **card** positioned in 2D space. Cards have relationships (edges), and the spatial layout carries meaning.

## Key Primitives

### 🃏 Cards

Every unit of conversation is a card. But not all cards are equal:

| Card Type | Description |
|-----------|-------------|
| **Prompt card** | User's question or instruction |
| **Response card** | Agent's reply, linked to its prompt |
| **Artifact card** | Code, images, docs — first-class objects, not inline blobs |
| **Reference card** | Pinned external context (files, URLs, data) |
| **Cluster card** | A named group — like a folder for related cards |

### 🔗 Edges

Cards connect via typed edges:
- **Reply-to** — conversational flow (prompt → response)
- **References** — "this card uses context from that card"
- **Contradicts** — the agent or user flagged a tension between two ideas
- **Evolves** — a later card supersedes or refines an earlier one

### 🧭 Regions

The canvas has soft **semantic zones** — not rigid panels, but areas that emerge organically:
- Drop a card about "auth" near other auth cards, and the region self-labels
- The agent can suggest: *"This feels related to your auth cluster — should I place it there?"*

## Interaction Patterns

### 1. Contextual Prompting

Instead of typing into a global input box, you **drop a prompt card near relevant context**. The agent automatically scopes its response to nearby cards. Ask a question near your database schema cards, and the agent knows you're asking about data modeling — no need to re-explain.

### 2. Fork & Explore

Drag a card to an empty region to start a **tangent**. The agent treats it as a new sub-conversation but retains awareness of the parent thread. You can explore two approaches side-by-side without polluting a single thread.

### 3. Gather & Synthesize

Select multiple cards, right-click → "Synthesize." The agent reads all selected cards and produces a **summary card** that distills the key points. Great for converging after divergent exploration.

### 4. Spatial Memory

The agent **remembers where things are**. You can say "go back to what we discussed in the top-left" and it understands. Position becomes a shared reference frame between human and agent.

### 5. Card Layering (Progressive Disclosure)

Cards have depth. The surface shows a title/summary. Click to expand to full content. Double-click to enter "focus mode" where that card becomes the viewport and its connections fan out around it.

## Agent Behaviors on the Canvas

The agent isn't just responding — it's **co-organizing**:

- **Auto-clustering**: Agent notices thematic overlap and suggests grouping
- **Gap detection**: Agent sees two clusters with no connection and asks *"Should these relate?"*
- **Conflict surfacing**: Agent draws a red edge between contradictory cards
- **Decay/archival**: Old, unreferenced cards fade visually — the agent can suggest archiving stale clusters
- **Layout negotiation**: Agent proposes rearrangements but never moves cards without permission

## The Input Model

The global input box **doesn't disappear** — it transforms:

- **Global prompt**: Type at the bottom bar → card appears at canvas center (or a smart default location)
- **Contextual prompt**: Click empty space on canvas → inline text field appears *at that position* → response appears nearby
- **Card-attached prompt**: Click a reply button on any card → follow-up anchors to that card

## Why This Works for Agentic Apps

1. **Tool calls become visible topology** — when the agent calls a tool, the tool call and result become linked cards. You can literally *see* the agent's reasoning chain laid out spatially.

2. **Multi-agent collaboration maps naturally** — each agent's contributions are color-coded. The canvas shows which agent produced what, and how their outputs feed into each other.

3. **Context window is tangible** — drag cards into/out of an "active context" region. The agent only "sees" what's in-region, giving users direct control over attention.

4. **Approval flows are spatial** — proposed actions appear in a staging area. Drag to "approved" region to execute, or drag to trash to reject.

## Deep Dive: Reasoning as Visible Topology

The killer feature of spatial chat isn't the canvas — it's that **agent thinking becomes a navigable landscape**.

### The Problem with Current Agentic UX

Today, when an agent works through a complex task, you see one of two things:
1. **A wall of text** — streaming tokens, tool calls buried in collapsible sections, results dumped inline. You're reading a log, not understanding a process.
2. **A black box** — the agent disappears for 30 seconds and hands you a result. You have no idea what it tried, what it rejected, or why it chose this path.

Both fail. Users need to **supervise** agents, not just receive their output. Supervision requires legibility, and legibility requires structure.

### Reasoning Topology

When an agent receives a complex prompt, it doesn't think linearly — it decomposes, explores, backtracks, synthesizes. Spatial chat makes this visible:

```
                    ┌─────────────┐
                    │ User Prompt │
                    └──────┬──────┘
                           │
                    ┌──────▼──────┐
                    │  Decompose  │
                    └──┬───────┬──┘
                       │       │
              ┌────────▼──┐ ┌──▼────────┐
              │ Sub-task A│ │ Sub-task B │
              └────┬──────┘ └──────┬────┘
                   │               │
            ┌──────▼──────┐  ┌─────▼─────┐
            │ Tool: grep  │  │ Tool: read │
            └──────┬──────┘  └─────┬─────┘
                   │               │
            ┌──────▼──────┐  ┌─────▼─────┐
            │  Result A   │  │  Result B  │
            └──────┬──────┘  └─────┬─────┘
                   │               │
                   └───────┬───────┘
                    ┌──────▼──────┐
                    │  Synthesize │
                    └──────┬──────┘
                    ┌──────▼──────┐
                    │   Answer    │
                    └─────────────┘
```

Each node is a **card on the canvas**. The user sees the agent's work *unfold spatially* in real time — sub-tasks fan out left and right, tool calls drop down, results bubble back up, and the synthesis pulls it all together.

### Card Types for Reasoning

| Card | Visual Treatment | Behavior |
|------|-----------------|----------|
| **Decomposition** | Dashed border, branch icon | Shows how the agent broke the problem apart |
| **Tool call** | Colored by tool type (blue=search, green=edit, orange=run) | Expandable: summary on surface, full I/O inside |
| **Dead end** | Grayed out, strikethrough title | Agent tried this path and abandoned it — preserved for transparency |
| **Decision point** | Diamond shape, like a flowchart | Shows what the agent chose and *what it didn't* — hover to see rejected options |
| **Synthesis** | Thick border, merge icon | Where multiple branches converge into a conclusion |

### Why Dead Ends Matter

This is the key insight: **showing what the agent rejected is as important as showing what it chose**. In linear chat, failed attempts are either hidden or create confusing "actually, let me try something else" messages. On the canvas, a dead-end branch is just a grayed-out fork — visible if you want it, ignorable if you don't.

A user supervising the agent can glance at the topology and immediately see:
- "It tried 3 approaches and picked this one" → confidence
- "It only tried one thing" → maybe I should suggest alternatives
- "It hit a dead end on the path I expected to work" → let me investigate

### Interaction: Steering the Reasoning

Because the reasoning is spatial, users can **intervene mid-thought**:

- **Prune**: Click a sub-task branch and collapse it → agent stops exploring that direction
- **Graft**: Drop a new card onto a branch → agent incorporates it as additional context for that sub-task
- **Redirect**: Drag an edge from a dead-end card to a new prompt card → "try this approach instead"
- **Promote**: Drag an intermediate result card out of the reasoning tree → it becomes a standalone artifact, persisted independent of the conversation

### Zoom Levels

The reasoning topology works at multiple scales:

| Zoom Level | What You See |
|------------|-------------|
| **Galaxy** | Clusters of conversations as nebulae. Each cluster is a topic or project. |
| **Constellation** | Individual conversation trees. You see the shape — wide trees = lots of exploration, narrow = focused execution. |
| **System** | Cards are readable. You see titles, types, status. The working zoom level. |
| **Surface** | Full card content. One card fills most of the viewport. Deep reading mode. |

At **Constellation** zoom, you can *literally see the shape of thinking*. A narrow deep tree means the agent went deep on one approach. A wide shallow tree means it explored broadly. Users develop intuition for these shapes over time — "that looks like a thorough investigation" vs "that looks like it gave up too early."

### Replaying Reasoning

Because the topology is a DAG (directed acyclic graph), you can **replay** it:
- Scrub a timeline slider to watch the agent's exploration unfold step by step
- Pause at any decision point and ask "why did you go left instead of right?"
- Fork from any historical point to explore "what if you had tried X instead?"

This turns the agent from an oracle into a **collaborator whose thought process is a shared artifact**.

### Collaborative Implications

When multiple humans supervise the same agent (team use case), the spatial layout becomes a **shared situation room**:
- Each team member's cursor is visible on the canvas
- One person can prune a branch while another grafts context onto a different branch
- The topology becomes a shared language: "look at the cluster in the top-right, I think we should redirect that"

## Technical Architecture

### High-Level Stack

```
┌─────────────────────────────────────────────────────┐
│                   Client (Browser)                   │
│                                                      │
│  ┌──────────┐  ┌──────────────┐  ┌───────────────┐  │
│  │ Canvas   │  │ Card Engine  │  │ Input Router  │  │
│  │ Renderer │  │ (React/Solid)│  │               │  │
│  │ (WebGL/  │  │              │  │ Global prompt │  │
│  │  Canvas) │  │ Card CRUD    │  │ Card prompt   │  │
│  │          │  │ Edge mgmt    │  │ Gesture mgmt  │  │
│  │ Viewport │  │ Layout algo  │  │ Keyboard nav  │  │
│  │ Culling  │  │ Selection    │  │               │  │
│  └────┬─────┘  └──────┬───────┘  └───────┬───────┘  │
│       │               │                  │           │
│  ┌────▼───────────────▼──────────────────▼───────┐   │
│  │              Local State (CRDT)                │   │
│  │     Canvas graph: nodes, edges, positions      │   │
│  │     Optimistic updates, undo/redo stack        │   │
│  └────────────────────┬──────────────────────────┘   │
│                       │                              │
└───────────────────────┼──────────────────────────────┘
                        │ WebSocket (bidirectional)
┌───────────────────────┼──────────────────────────────┐
│                  Server / Backend                     │
│                       │                              │
│  ┌────────────────────▼──────────────────────────┐   │
│  │              Sync Engine (CRDT)                │   │
│  │     Merge concurrent edits from N clients      │   │
│  │     + agent writes                             │   │
│  └────────┬──────────────────────┬───────────────┘   │
│           │                      │                   │
│  ┌────────▼────────┐   ┌────────▼────────────────┐   │
│  │  Canvas Store   │   │   Agent Orchestrator    │   │
│  │  (Persistent)   │   │                         │   │
│  │                 │   │   Context assembler      │   │
│  │  Graph DB or    │   │   Tool executor          │   │
│  │  Document store │   │   Card emitter           │   │
│  │  + spatial index│   │   Reasoning DAG builder  │   │
│  └─────────────────┘   └─────────────────────────┘   │
│                                                      │
└──────────────────────────────────────────────────────┘
```

### Data Model: The Canvas Graph

The entire canvas is a **directed property graph**. Everything is a node or an edge.

```typescript
interface CanvasNode {
  id: string;
  type: CardType;
  position: { x: number; y: number };
  dimensions: { width: number; height: number };
  content: CardContent;         // type-specific payload
  metadata: {
    createdBy: string;          // user ID or agent ID
    createdAt: number;
    status: 'active' | 'dead-end' | 'archived';
    confidenceScore?: number;   // 0-1, for agent-generated cards
    zoomVisibility: ZoomLevel;  // minimum zoom level to render
  };
}

type CardType =
  | 'prompt'          // user input
  | 'response'        // agent reply
  | 'artifact'        // code, doc, image
  | 'reference'       // pinned external context
  | 'cluster'         // named group
  | 'decomposition'   // agent broke a problem apart
  | 'tool-call'       // agent invoked a tool
  | 'decision-point'  // agent chose between options
  | 'synthesis'       // agent merged branches
  | 'dead-end';       // agent abandoned this path

interface CanvasEdge {
  id: string;
  source: string;     // node ID
  target: string;     // node ID
  type: EdgeType;
  metadata: {
    createdBy: string;
    createdAt: number;
    weight?: number;   // relevance strength for 'references' edges
  };
}

type EdgeType =
  | 'reply-to'        // conversational flow
  | 'references'      // semantic dependency
  | 'contradicts'     // tension between cards
  | 'evolves'         // supersedes/refines
  | 'decomposes-to'   // parent → sub-task
  | 'tool-io'         // tool call → result
  | 'synthesizes';    // inputs → synthesis
```

### Rendering: Hybrid Canvas Architecture

Pure DOM won't scale to hundreds of cards. Pure WebGL loses text rendering quality and accessibility. The answer is a **hybrid**:

| Layer | Technology | Responsibility |
|-------|-----------|----------------|
| **Background** | WebGL / Canvas2D | Grid, regions, ambient effects (glow, decay fade), minimap |
| **Edges** | SVG overlay | Typed edges with labels, animations (data flowing along edges during tool calls) |
| **Cards** | DOM elements with CSS transforms | Rich text, interactive content, accessibility, form inputs |
| **HUD** | Fixed-position DOM | Toolbar, zoom controls, search, global prompt bar |

Cards are positioned via `transform: translate(x, y) scale(z)` on DOM nodes. A **viewport culling** system only mounts cards visible in the current viewport + a buffer zone. This is the same technique game engines use — only render what the camera sees.

```typescript
interface Viewport {
  center: { x: number; y: number };
  zoom: number;           // 0.05 (galaxy) → 2.0 (surface)
  bounds: BoundingBox;    // computed from center + zoom + screen size
}

// On every frame / pan / zoom:
function getVisibleNodes(graph: CanvasGraph, viewport: Viewport): CanvasNode[] {
  // Spatial index query (R-tree) — O(log n) not O(n)
  return graph.spatialIndex.query(viewport.bounds.expand(BUFFER));
}
```

**R-tree spatial index** is critical. Naively checking every card against the viewport is O(n). An R-tree makes it O(log n), which keeps panning smooth even with 10k+ cards.

### Context Assembly: Proximity → LLM Context

This is the most novel architectural piece. **Spatial position determines what the agent sees.**

When a user creates a prompt card at position (x, y), the system assembles context by:

```typescript
function assembleContext(promptCard: CanvasNode, graph: CanvasGraph): LLMContext {
  const layers: ContextLayer[] = [];

  // Layer 1: Direct edges (always included)
  const directlyConnected = graph.getConnected(promptCard.id, { depth: 1 });
  layers.push({ cards: directlyConnected, priority: 'required' });

  // Layer 2: Spatial neighbors (proximity-based)
  const nearby = graph.spatialIndex.queryRadius(
    promptCard.position,
    PROXIMITY_RADIUS
  );
  layers.push({ cards: nearby, priority: 'high' });

  // Layer 3: Same-cluster siblings
  const cluster = graph.getContainingCluster(promptCard.id);
  if (cluster) {
    layers.push({ cards: cluster.children, priority: 'medium' });
  }

  // Layer 4: Graph-connected (follow edges outward)
  const graphNeighbors = graph.getConnected(promptCard.id, { depth: 3 });
  layers.push({ cards: graphNeighbors, priority: 'low' });

  // Assemble and trim to context window budget
  return buildPrompt(layers, TOKEN_BUDGET);
}
```

The key insight: **proximity is a continuous relevance signal**. Cards right next to the prompt are almost certainly relevant. Cards across the canvas probably aren't. This replaces the clunky "attach files to context" pattern with an intuitive spatial metaphor — *put things near each other if they're related*.

### Agent → Canvas: The Card Emitter

The agent doesn't return plain text — it emits **structured card events** via a streaming protocol:

```typescript
type AgentEvent =
  | { type: 'card:create'; card: Partial<CanvasNode>; parentEdge?: EdgeType }
  | { type: 'card:update'; id: string; patch: Partial<CanvasNode> }
  | { type: 'card:status'; id: string; status: 'active' | 'dead-end' }
  | { type: 'edge:create'; edge: CanvasEdge }
  | { type: 'layout:suggest'; nodes: string[]; arrangement: LayoutHint }
  | { type: 'stream:token'; cardId: string; token: string }  // streaming text into a card
  | { type: 'reasoning:branch'; from: string; branches: string[] }
  | { type: 'reasoning:merge'; from: string[]; into: string };
```

The agent orchestrator wraps the raw LLM output in this protocol. When the LLM calls a tool, the orchestrator emits:
1. `card:create` for the tool-call card
2. `stream:token` events as the tool runs (for streaming results)
3. `card:create` for the result card
4. `edge:create` linking them with a `tool-io` edge

The **layout engine** on the client receives `layout:suggest` hints and positions new cards using a force-directed algorithm constrained to the suggested arrangement. The agent says "put these cards in a tree layout below card X" and the physics engine makes it happen smoothly.

### Collaboration: CRDTs, Not OT

Multiple users + agent all writing to the same canvas = conflict resolution problem. **CRDTs (Conflict-free Replicated Data Types)** are the right fit because:

- No central coordinator needed — each participant (human or agent) applies operations locally and syncs
- The graph operations (add node, move node, add edge, update content) map cleanly to known CRDT types
- The agent is just another participant — its writes merge the same way human writes do

```
CRDT type mapping:
  - Node set          → Observed-Remove Set (OR-Set)
  - Node positions    → Last-Writer-Wins Register (LWW per node)
  - Edge set          → OR-Set
  - Card content      → Peritext (rich text CRDT) or Yjs Y.Text
  - Undo stack        → Per-user causal history
```

**Yjs** is the pragmatic choice — battle-tested, supports all these types, has a WebSocket sync provider, and integrates with popular editors for rich text inside cards.

### Persistence: Layered Storage

```
┌─────────────────────────────┐
│  Hot:  In-memory CRDT doc   │  ← active editing, <100ms sync
├─────────────────────────────┤
│  Warm: SQLite / Turso       │  ← canvas metadata, spatial index
│        (graph as tables)    │     recent sessions, fast queries
├─────────────────────────────┤
│  Cold: Object storage       │  ← archived canvases, snapshots
│        + search index       │     full-text search across all
│                             │     historical cards
└─────────────────────────────┘
```

The graph is stored relationally for queryability:

```sql
CREATE TABLE nodes (
  id TEXT PRIMARY KEY,
  canvas_id TEXT NOT NULL,
  type TEXT NOT NULL,
  x REAL NOT NULL,
  y REAL NOT NULL,
  width REAL,
  height REAL,
  content JSONB,
  metadata JSONB,
  created_by TEXT,
  created_at TIMESTAMP DEFAULT NOW()
);

CREATE TABLE edges (
  id TEXT PRIMARY KEY,
  canvas_id TEXT NOT NULL,
  source_id TEXT REFERENCES nodes(id),
  target_id TEXT REFERENCES nodes(id),
  type TEXT NOT NULL,
  metadata JSONB,
  created_at TIMESTAMP DEFAULT NOW()
);

-- Spatial index for proximity queries
CREATE INDEX idx_nodes_spatial ON nodes USING gist (
  point(x, y)
);
```

### Performance Budget

| Metric | Target | Strategy |
|--------|--------|----------|
| Pan/zoom FPS | 60fps | Viewport culling + R-tree |
| Card render count | ≤200 visible at once | LOD: distant cards → dots/labels |
| Agent card emit latency | <50ms to appear | Optimistic local insert, sync async |
| Context assembly | <200ms | Pre-computed spatial index, cached graph traversals |
| Canvas load (1000 cards) | <2s | Progressive load: viewport first, then outward |
| Collaboration sync | <100ms peer-to-peer | CRDT with WebSocket, no server round-trip for local ops |

### Key Technical Risks

1. **Layout thrashing**: When the agent emits 20 cards in 2 seconds, the force-directed layout could jitter. Mitigation: batch layout updates, animate smoothly, lock user-positioned cards.

2. **Context assembly cost**: Traversing the graph + spatial queries on every prompt could be expensive. Mitigation: maintain a pre-computed "context neighborhood" that updates incrementally as cards move.

3. **Rich text in cards at scale**: Hundreds of DOM nodes with rich text editors are heavy. Mitigation: only mount editors for selected/hovered cards; render others as static HTML or even canvas-drawn text at far zoom.

4. **CRDT document size**: Long-lived canvases could accumulate large CRDT histories. Mitigation: periodic snapshots that compress history, with full history available on-demand.

## Open Design Questions

- **Performance**: How to handle canvases with hundreds of cards? Probably LOD (level-of-detail) rendering — distant clusters collapse into labeled dots
- **Onboarding**: How do you teach spatial interaction to users trained on linear chat? Maybe start with a single-column "rail" view that users can gradually pull apart into 2D
- **Mobile**: Spatial UIs are hard on small screens — maybe a collapsible outline view as a mobile fallback
- **Persistence**: Is each canvas a "session"? Can you merge canvases? Link across them?
