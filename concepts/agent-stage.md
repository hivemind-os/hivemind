# Multi-Agent Stage

## The Core Idea

When multiple agents collaborate on a task, today's UIs collapse everything into a single thread — you can't tell who did what, who asked whom, or how work was delegated. The **Multi-Agent Stage** makes agent collaboration **visible, spatial, and steerable**.

Think of it as a **war room** or **theater stage** where each agent is a visible character with a role, position, and set of relationships. The user is the director.

## Mental Model

Mashup of:
- **A film set** — director (user) orchestrates actors (agents) who each have a role
- **A network operations center** — operators see all systems, their status, and the data flowing between them
- **Multiplayer game UI** — each character has a portrait, status bar, and activity feed

## Why Current Multi-Agent UX Fails

| Problem | What happens today |
|---------|-------------------|
| **Attribution collapse** | Agent A delegates to Agent B, but the user just sees one stream of text. Who decided what? |
| **Invisible delegation** | The orchestrator agent spawns 4 sub-agents. The user has no idea this happened until they see the bill. |
| **No steering** | Once an agent delegates, the user can't redirect the sub-agent without going through the orchestrator. |
| **Context blindness** | Each agent operates with different context, but the user can't see what each agent "knows." |
| **Blame ambiguity** | Something went wrong. Which agent caused it? Good luck tracing that in a linear log. |

## The Stage Metaphor

### The Layout

```
┌──────────────────────────────────────────────────────────────┐
│                        THE STAGE                             │
│                                                              │
│    ┌─────────┐          ┌─────────┐          ┌─────────┐    │
│    │ 🏗️      │ ──────▶  │ 🔍      │          │ 📝      │    │
│    │ Planner │ context  │ Research│ ──────▶  │ Writer  │    │
│    │         │          │         │ findings │         │    │
│    │ ACTIVE  │          │ ACTIVE  │          │ WAITING │    │
│    │ ████░░░ │          │ ██████░ │          │ ░░░░░░░ │    │
│    └─────────┘          └─────────┘          └─────────┘    │
│         │                                         ▲          │
│         │              ┌─────────┐                │          │
│         └────────────▶ │ 🧪      │ ───────────────┘          │
│            sub-task    │ Coder   │    code artifact           │
│                        │         │                           │
│                        │ ACTIVE  │                           │
│                        │ ███░░░░ │                           │
│                        └─────────┘                           │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ 🎬 Director's Console (You)                            │  │
│  │ > "Build a REST API for user management"               │  │
│  │                                                        │  │
│  │ [Broadcast to all] [Whisper to...▾] [Pause all] [Recast]│ │
│  └────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────┘
```

Each agent is a **character on stage** with:
- **Avatar & name** — visual identity (icon, color, name)
- **Role label** — what this agent specializes in (planner, researcher, coder, reviewer)
- **Status** — `ACTIVE` / `WAITING` / `BLOCKED` / `DONE` / `ERROR`
- **Progress bar** — how far through its current sub-task
- **Activity pulse** — subtle animation when actively processing (like a breathing glow)

### Edges Between Agents

The arrows between agents are **live data flows**, not static lines:

| Edge Type | Visual | Meaning |
|-----------|--------|---------|
| **Delegation** | Solid arrow, animated particles flowing | Agent A assigned a sub-task to Agent B |
| **Context share** | Dashed line, document icon | Agent A shared context/files with Agent B |
| **Artifact pass** | Thick arrow, artifact thumbnail on the edge | Agent A produced something Agent B consumes |
| **Feedback loop** | Bidirectional arrows, pulsing | Two agents iterating — e.g., coder ↔ reviewer |
| **Blocked-by** | Red line, stop icon | Agent A is waiting on Agent B |

When data flows along an edge, you see **animated particles** moving from source to target — like watching packets traverse a network. This gives an immediate sense of activity and direction.

## Agent Cards (Expanded View)

Click any agent to expand its card and see the full picture:

```
┌─────────────────────────────────────────┐
│ 🔍 Research Agent                [✕]    │
│ Role: Deep research & fact-finding      │
│ Model: Claude Sonnet 4.5               │
│ Status: ACTIVE — searching codebase     │
│ ████████████░░░░░░░░  58%               │
│                                         │
│ ┌─── Context Window ───────────────┐    │
│ │ 📄 user-api-spec.md      2.1k tk │    │
│ │ 📄 existing-schema.sql   890 tk  │    │
│ │ 💬 Planner's brief       340 tk  │    │
│ │ 🔧 grep results (3)     1.2k tk  │    │
│ │                                  │    │
│ │ Total: 4,530 / 128,000 tokens    │    │
│ │ [+ Add context] [🗑️ Remove]      │    │
│ └──────────────────────────────────┘    │
│                                         │
│ ┌─── Activity Log ─────────────────┐    │
│ │ 21:14  Received brief from       │    │
│ │        Planner                    │    │
│ │ 21:14  Tool: grep "user model"   │    │
│ │ 21:15  Tool: read schema.sql     │    │
│ │ 21:15  Tool: grep "auth middle*" │    │
│ │ 21:16  Composing findings...     │    │
│ └──────────────────────────────────┘    │
│                                         │
│ ┌─── Cost ─────────────────────────┐    │
│ │ Input: 12.4k tokens  ($0.003)    │    │
│ │ Output: 3.1k tokens  ($0.002)    │    │
│ │ Tools: 3 calls                   │    │
│ └──────────────────────────────────┘    │
│                                         │
│ [⏸️ Pause] [🔄 Restart] [🗑️ Kill]      │
│ [💬 Whisper] [📋 Recast role]           │
└─────────────────────────────────────────┘
```

### What "Recast" Means

This is a key concept: the user can **recast an agent's role mid-task**. If the Research agent is struggling, you can:
- Swap its underlying model (Haiku → Sonnet → Opus)
- Change its system prompt / role description
- Redirect its context (drag different files onto its card)
- Replace it with a different agent type entirely

It's like a director replacing an actor mid-scene — the new agent picks up from the same point with the same context.

## Director's Console: Steering Multi-Agent Work

The user's input is more nuanced than a single text box:

### Communication Modes

| Mode | How it works | When to use |
|------|-------------|-------------|
| **Broadcast** | Message goes to all agents simultaneously | High-level direction changes, new constraints |
| **Whisper** | Message goes to one specific agent | Correcting a single agent, providing targeted context |
| **Interrupt** | Pauses all agents, broadcasts a message, waits for acknowledgment | "Stop — requirements changed" |
| **Redirect** | Re-routes one agent's output to a different agent | "Actually, send those findings to Coder, not Writer" |

### Drag-and-Drop Interactions

The stage is fully interactive:

- **Drag a file onto an agent** → adds it to that agent's context
- **Drag an agent's output card onto another agent** → shares that artifact as context
- **Drag an edge to re-route it** → changes the delegation/data flow graph
- **Drag an agent off-stage** → removes it from the current task (with confirmation)
- **Drag a new agent from a roster panel onto the stage** → adds a specialist to the team

## Agent Topologies

Different tasks call for different arrangements. The stage supports several **topology patterns**:

### 1. Pipeline (Sequential)

```
User → Planner → Researcher → Coder → Reviewer → User
```
Each agent completes its work then hands off to the next. Simple, predictable, but slow.

### 2. Fan-Out / Fan-In

```
              ┌→ Researcher A ─┐
User → Planner┼→ Researcher B ─┼→ Synthesizer → User
              └→ Researcher C ─┘
```
Planner decomposes, multiple agents work in parallel, synthesizer merges results. Fast for parallelizable tasks.

### 3. Feedback Loop

```
User → Coder ⇄ Reviewer → User
```
Two agents iterate until quality bar is met. The user watches the ping-pong and can intervene.

### 4. Hierarchy

```
User → Orchestrator → Sub-orchestrator A → Worker A1
                                         → Worker A2
                    → Sub-orchestrator B → Worker B1
```
Deep delegation trees for complex projects. The stage shows this as nested clusters.

### 5. Swarm

```
User → Swarm Controller ──→ Agent 1 (autonomous)
                          ──→ Agent 2 (autonomous)
                          ──→ Agent 3 (autonomous)
                          ──→ Agent N (autonomous)
```
Many agents working independently on sub-problems. The stage shows them as a cloud with individual status indicators.

The user can **switch topologies mid-task** — start with a pipeline, realize it's too slow, drag agents into a fan-out arrangement.

## Live Telemetry Dashboard

A collapsible bottom panel shows real-time metrics:

```
┌──────────────────────────────────────────────────────────────┐
│ 📊 Telemetry                                                 │
│                                                              │
│ Agents active: 4/4    Total tokens: 48.2k    Cost: $0.031   │
│ Elapsed: 2m 14s       Tool calls: 17         Errors: 0      │
│                                                              │
│ Token flow (last 60s):                                       │
│ ████████████████████░░░░░░░░░░ Planner (done)               │
│ ██████████████████████████████ Researcher (active)           │
│ ░░░░░░░░░░░░░░░░░░░░░░░░░░░░ Writer (waiting)              │
│ ████████████████░░░░░░░░░░░░░ Coder (active)                │
│                                                              │
│ [Export trace] [Cost alert: set budget ▾]                    │
└──────────────────────────────────────────────────────────────┘
```

### Cost Controls as First-Class UI

Multi-agent systems can burn through tokens fast. The stage makes cost **tangible**:

- Per-agent cost displayed on each card
- Running total in the telemetry bar
- **Budget fence**: set a dollar limit. When 80% is reached, all agents pause and the user decides whether to continue
- **Cost projection**: based on current token velocity, the stage estimates total cost at completion
- Color coding: green (under budget) → yellow (approaching limit) → red (over budget)

## The Roster Panel

A sidebar listing available agent types you can drag onto the stage:

```
┌─── Agent Roster ──────────┐
│                            │
│ 🏗️ Planner                │
│   Decomposes tasks         │
│                            │
│ 🔍 Researcher              │
│   Deep search & analysis   │
│                            │
│ 📝 Writer                  │
│   Prose, docs, summaries   │
│                            │
│ 🧪 Coder                   │
│   Implementation           │
│                            │
│ 🔬 Reviewer                │
│   Code review & QA         │
│                            │
│ 🧮 Data Analyst            │
│   SQL, charts, insights    │
│                            │
│ 🎨 Designer                │
│   UI/UX mockups            │
│                            │
│ ┌────────────────────────┐ │
│ │ + Create custom agent  │ │
│ └────────────────────────┘ │
└────────────────────────────┘
```

Users can create **custom agents** with:
- A name and avatar
- A system prompt / role description
- A default model
- A set of allowed tools
- Pre-loaded context (files, docs, URLs)

Custom agents are saved and reusable across sessions — like building your team.

## Replay & Post-Mortem

After a multi-agent task completes, the stage becomes a **replay viewer**:

- Scrub a timeline to watch the collaboration unfold
- See when each agent was active, what it received, what it produced
- Identify bottlenecks (which agent held everyone up?)
- Trace any output back to the agent and input that produced it
- **Export as a trace** — sharable artifact for debugging or knowledge sharing

This is invaluable for:
- Debugging why an agent swarm produced bad output
- Optimizing team composition for recurring tasks
- Training new users on how agent collaboration works

## Relationship to Spatial Chat

The Multi-Agent Stage can exist **inside** the Spatial Chat canvas:
- Each agent's work products are cards on the canvas
- The stage view is a **lens** — a filtered view of the canvas showing only agent relationships and status
- Toggle between "stage view" (agent-centric) and "canvas view" (content-centric)
- Or run them side-by-side: stage on the left, canvas on the right

## Open Design Questions

- **Agent autonomy spectrum**: How much should agents auto-coordinate vs. require user orchestration? Probably a slider: "fully manual" ↔ "fully autonomous"
- **Noise management**: With 5+ agents active, the stage could feel overwhelming. How to surface what matters? Maybe an "attention" system that highlights agents needing user input
- **Trust calibration**: Users need to learn which agent types/models are reliable for which tasks. Should the stage show historical success rates?
- **Failure cascades**: When one agent in a pipeline fails, how does the stage communicate the blast radius? Maybe downstream agents turn yellow with a "upstream dependency failed" label
- **Shared memory vs. isolated context**: Should agents share a common memory space, or should all sharing be explicit (via edges)? Explicit is more legible but adds friction
