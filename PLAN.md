# HiveMind OS — Implementation Plan

This document translates the [HiveMind OS Specification](./SPEC.md) into an actionable, phased implementation plan. Each phase builds on the previous, producing a usable (if incomplete) system at every milestone.

---

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                          HiveMind OS Daemon (Rust)                        │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐           │
│  │  Config   │  │  Audit   │  │  Event   │  │ Credential│           │
│  │  Loader   │  │  Logger  │  │  Bus     │  │  Vault    │           │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘           │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    Classification Engine                      │   │
│  │  Labellers → Gate → Override Policy → Audit                  │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐           │
│  │  Model    │  │  MCP     │  │ Knowledge│  │ Embedded │           │
│  │  Router   │  │  Client  │  │  Graph   │  │ Models   │           │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘           │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    Agentic Loop Engine                        │   │
│  │  Strategies → Middleware → Workflow Engine → DSL Runtime      │   │
│  └──────────────────────────────────────────────────────────────┘   │
│                                                                     │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐           │
│  │ Scheduler│  │  Roles & │  │ Peering  │  │ Messaging│           │
│  │          │  │  Agents  │  │ Transport│  │ Bridges  │           │
│  └──────────┘  └──────────┘  └──────────┘  └──────────┘           │
│                                                                     │
│  ┌──────────────────────────────────────────────────────────────┐   │
│  │                    Local API (Socket + HTTP/WS)               │   │
│  └──────────────────────────────────────────────────────────────┘   │
└──────┬──────────────┬──────────────┬──────────────┬─────────────────┘
       │              │              │              │
 ┌─────▼─────┐  ┌─────▼─────┐  ┌────▼────┐  ┌─────▼──────────┐
 │ Tauri UI  │  │ Web       │  │  CLI    │  │  Messaging     │
 │           │  │ Console   │  │         │  │  (Slack/Discord)│
 └───────────┘  └───────────┘  └─────────┘  └────────────────┘
```

---

## Crate Structure

```
hivemind-os/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── hive-daemon/             # Main daemon binary
│   ├── hive-cli/                # CLI binary
│   ├── hive-core/               # Shared types, traits, config
│   ├── hive-api/                # Local API server (HTTP/WS + socket)
│   ├── hive-classification/     # Data classification engine
│   ├── hive-providers/          # Model provider adapters
│   ├── hive-mcp/                # MCP client implementation
│   ├── hive-knowledge/          # Knowledge graph engine
│   ├── hive-embedded-models/    # In-process model inference
│   ├── hive-loop/               # Agentic loop engine + DSL runtime
│   ├── hive-workflow/           # Workflow engine + state persistence
│   ├── hive-scheduler/          # Background task scheduler
│   ├── hive-agents/             # Roles, instances, inter-agent comms
│   ├── hive-peering/            # Peer identity, transport, sync
│   ├── hive-messaging/          # External messaging bridges
│   ├── hive-skills/             # Agent Skills loader
│   ├── hive-tools/              # Built-in tool implementations
│   └── hive-crypto/             # Encryption, signing, keychain
├── tauri-app/                    # Tauri v2 desktop UI
│   ├── src-tauri/                # Tauri Rust glue (thin — connects to daemon)
│   └── src/                      # Frontend (React/Solid/Svelte + TypeScript)
├── web-console/                  # Browser-based UI (shares frontend code)
└── docs/                         # Developer documentation
```

---

## Pre-Phase: Proof of Concepts

**Goal:** Validate the six highest-risk technical bets with minimal standalone spikes before committing to the full architecture. Each PoC is an independent Cargo project under `poc/`. All PoCs should be completed before beginning Phase 0.

---

### PoC 1 🔴 — Tauri Daemon-First Architecture

**Risk:** Tauri assumes it *is* the app. Our spec requires a standalone Rust daemon that runs independently, with the Tauri UI as one of several equal clients. If this separation doesn't work cleanly, it affects the entire architecture.

**What to build:**
A two-process setup:
1. `poc/daemon/` — A Rust binary (Tokio) that listens on a Unix socket (macOS/Linux) / named pipe (Windows) and exposes a simple JSON-RPC API (`ping`, `get_status`, `shutdown`). Optionally also listens on `localhost:9100` for HTTP/WS.
2. `poc/tauri-client/` — A Tauri v2 + SolidJS app that connects to the daemon's socket on startup, displays status, and sends commands.

**Key crates:** `tokio`, `hyper`/`axum`, `interprocess`, `tauri v2`, `solid-js`

**Pass criteria:**
- [ ] Daemon starts independently via `cargo run -p poc-daemon`
- [ ] Tauri app starts, connects to the already-running daemon, displays `pong` response
- [ ] Tauri window closes → daemon keeps running (verify with `curl localhost:9100/ping`)
- [ ] Daemon is started if not already running when Tauri app launches
- [ ] System tray icon persists after window close (Tauri `tray-icon` feature)
- [ ] SolidJS frontend renders, calls `@tauri-apps/api` invoke → Tauri Rust command → daemon socket → response displayed
- [ ] Works on Windows (named pipe) and macOS/Linux (Unix socket)

**Fallback if fails:** Run daemon logic inside Tauri's core process, use `keep_alive` on window close + system tray. Less clean separation but workable.

---

### PoC 2 🔴 — SQLite Property Graph + FTS5 + sqlite-vec

**Risk:** We're using SQLite as a graph database, full-text search engine, and vector store simultaneously. Need to verify that all three work together in one database from Rust, and that recursive CTE classification propagation performs at scale.

**What to build:**
`poc/knowledge-graph/` — A Rust binary that:
1. Opens a SQLite database with `rusqlite` (bundled feature)
2. Loads `sqlite-vec` via `sqlite3_auto_extension`
3. Creates the property-graph schema (nodes, edges, properties tables)
4. Creates an FTS5 virtual table on node content
5. Creates a `vec_nodes` virtual table with `float[384]` embeddings
6. Seeds 10,000 nodes with random hierarchy (5 levels deep), random data classifications, text content, and random 384-dim embeddings
7. Runs performance benchmarks

**Key crates:** `rusqlite` (bundled), `sqlite-vec`, `zerocopy`, `rand`, `criterion`

**Pass criteria:**
- [ ] All three features (graph schema, FTS5, sqlite-vec) coexist in one `.db` file
- [ ] Insert 10K nodes + 15K edges + 10K embeddings completes in < 5 seconds
- [ ] Recursive CTE for `effective_class` (traversing 5-level ancestor chain) completes in < 50ms for a single node
- [ ] Batch classification check on 1,000 nodes (with ancestor propagation) completes in < 500ms
- [ ] FTS5 `MATCH` query returns results in < 10ms
- [ ] sqlite-vec KNN query (`WHERE embedding MATCH ? ORDER BY distance LIMIT 10`) returns results in < 50ms with 10K vectors
- [ ] Combined query: FTS5 filter → sqlite-vec re-rank on matched subset works correctly
- [ ] Classification-filtered vector search: add `WHERE data_class <= 1` before KNN, verify restricted nodes excluded
- [ ] Works identically on Windows and macOS/Linux

**Fallback if fails:** If sqlite-vec integration is problematic → use `usearch` crate for vector search in a separate index file (more complex but proven). If recursive CTEs are too slow → precompute `effective_class` on write via triggers.

---

### PoC 3 🔴 — QuickJS Sandbox in Rust (Async Interop)

**Risk:** Custom agentic loop stages are authored in TypeScript/JavaScript and executed in a sandboxed QuickJS runtime. The critical question: can async Rust (the daemon) bridge cleanly with QuickJS, especially for `ctx.tools.call()` which must call back into Rust and await a result?

**What to build:**
`poc/quickjs-sandbox/` — A Rust binary that:
1. Creates a `rquickjs::AsyncRuntime` and `AsyncContext`
2. Injects a `ctx` object with methods:
   - `ctx.tools.call(name, args)` → returns a Promise that resolves with a Rust-computed result
   - `ctx.model.complete(prompt)` → returns a Promise (simulated with a delay)
   - `ctx.state.get(key)` / `ctx.state.set(key, value)` → sync read/write to a Rust HashMap
3. Executes a sample JS stage script that uses all three APIs
4. Verifies that fs, network, and process APIs are NOT available

**Key crates:** `rquickjs` (with `futures` feature), `tokio`

**Sample test script:**
```javascript
export default async function(ctx) {
  const files = await ctx.tools.call("list_files", { path: "/home" });
  const summary = await ctx.model.complete(`Summarize: ${JSON.stringify(files)}`);
  ctx.state.set("last_summary", summary);
  return { result: summary, file_count: files.length };
}
```

**Pass criteria:**
- [ ] JS function calls `ctx.tools.call()` → Rust async function executes → Promise resolves in JS with correct value
- [ ] JS function calls `ctx.model.complete()` → simulated 200ms delay in Rust → Promise resolves
- [ ] `ctx.state.get/set` round-trips correctly between JS and Rust
- [ ] Function returns a value that Rust can deserialize into a typed struct
- [ ] `require('fs')`, `require('net')`, `fetch()`, `Deno`, `process` are all `undefined` or throw
- [ ] Execution timeout: a script with `while(true){}` is killed after configurable timeout (e.g. 5 seconds)
- [ ] Memory limit: a script that allocates unbounded arrays is killed when exceeding limit
- [ ] Two independent scripts can run in separate contexts without sharing state
- [ ] Works on Windows + macOS/Linux

**Fallback if fails:** If `rquickjs` async interop is unreliable → use synchronous QuickJS with a dedicated OS thread and channel-based communication to async Rust. More complexity, but decouples the two runtimes.

---

### PoC 4 🟡 — Embedded Model Inference (llama-cpp-rs)

**Risk:** Cross-platform native builds with GPU acceleration are historically painful. Need to verify that `llama-cpp-rs` builds and loads a GGUF model on all target platforms without requiring users to install CUDA/Metal SDKs manually.

**What to build:**
`poc/embedded-model/` — A Rust binary that:
1. Loads a small GGUF model (e.g. TinyLlama-1.1B-Q4_K_M, ~600MB)
2. Runs a simple completion: `"The capital of France is"` → expects `"Paris"`
3. Reports backend used (Metal/CUDA/CPU), tokens/second, memory usage

**Key crates:** `llama-cpp-rs` (with `metal` / `cuda` features), `hf-hub` (for model download)

**Pass criteria:**
- [ ] `cargo build --release` succeeds on macOS (no Xcode full install, just CLI tools)
- [ ] `cargo build --release` succeeds on Windows (with Visual Studio Build Tools)
- [ ] `cargo build --release` succeeds on Linux (with gcc/clang)
- [ ] Model loads and generates coherent text on CPU fallback (all platforms)
- [ ] Metal acceleration activates automatically on macOS Apple Silicon
- [ ] CUDA acceleration activates when CUDA toolkit is present on Windows/Linux
- [ ] Model can be loaded and unloaded without memory leaks (run 10 load/unload cycles, check RSS)
- [ ] Inference is cancelable (abort mid-generation via a CancellationToken or similar)

**Fallback if fails:** If `llama-cpp-rs` build complexity is too high → use `candle` (pure Rust, no C++ dependency, but potentially slower). If neither works → defer embedded models and rely on local Ollama via OpenAI-compatible API (already supported as a provider).

---

### PoC 5 🟡 — MCP Client in Rust (stdio + SSE)

**Risk:** The Rust MCP SDK (`rmcp`) is Tier 2. We need to verify it supports the features we need: tool calling, resource reading, stdio transport for local servers, SSE transport for remote servers, and the notifications API.

**What to build:**
`poc/mcp-client/` — A Rust binary that:
1. Launches an MCP server as a child process via stdio (use the reference `@modelcontextprotocol/server-everything` npm package, or a simple test server)
2. Performs capability negotiation (`initialize` / `initialized`)
3. Lists available tools, calls one, verifies the result
4. Lists resources, reads one
5. Subscribes to notifications, verifies receipt

**Key crates:** `rmcp`, `tokio`

**Pass criteria:**
- [ ] Successfully connects to a stdio MCP server and completes handshake
- [ ] `tools/list` returns the server's tool inventory
- [ ] `tools/call` invokes a tool and returns the expected result
- [ ] `resources/list` and `resources/read` work correctly
- [ ] Notification subscription works (server sends a notification, client receives it)
- [ ] SSE transport connects to an HTTP-based MCP server (if available)
- [ ] Graceful shutdown: client sends `shutdown` and the child process exits cleanly
- [ ] Error handling: client recovers from a crashed server (detects broken pipe, reports error)

**Fallback if fails:** If `rmcp` is too immature → implement MCP protocol directly using `serde_json` + `tokio::process::Command` for stdio and `reqwest` + `eventsource-client` for SSE. The protocol is JSON-RPC over these transports, so a minimal implementation is feasible.

---

### PoC 6 🟡 — SolidJS + Tauri v2 Integration

**Risk:** Tauri's official scaffolding and examples favor React/Vue/Svelte. SolidJS has community templates but we need to verify the full dev experience: HMR, Tauri command invocation, builds, and platform webview compatibility.

**What to build:**
`poc/tauri-solid/` — A Tauri v2 app scaffolded with SolidJS + Vite + TypeScript + Tailwind CSS:
1. A counter component with reactive state
2. A "Call Daemon" button that invokes a Tauri command (Rust → returns data)
3. A chat-style message list that receives events from Rust via `listen()`
4. Tailwind CSS styling with dark/light theme toggle

**Key tools:** `npm create tauri-app@latest` or `degit` a community template, `solid-js`, `vite`, `tailwindcss`, `@tauri-apps/api`

**Pass criteria:**
- [ ] `npm run dev` starts Vite dev server with HMR — SolidJS components update live
- [ ] `npm run tauri dev` opens Tauri window with SolidJS app rendered
- [ ] Tauri `invoke('greet', { name: 'HiveMind OS' })` from SolidJS → Rust command → response displayed
- [ ] Tauri `listen('daemon-event')` receives events emitted from Rust backend
- [ ] Production build (`npm run tauri build`) produces a working installer
- [ ] Tailwind styles render correctly in the webview on Windows (WebView2) and macOS (WebKit)
- [ ] No console errors related to `@tauri-apps/api` module resolution
- [ ] Bundle size of the frontend JS is under 200KB gzipped

**Fallback if fails:** If SolidJS has critical webview incompatibilities → switch to Svelte (next-best option for small bundles + no virtual DOM). If Svelte also fails → React is the safe fallback.

---

### PoC Execution Order

The PoCs have no hard dependencies on each other and **can be developed in parallel**. However, if doing them sequentially, the recommended order is:

```
Priority:  PoC 1 (Daemon) ──→ PoC 6 (Tauri+Solid) ──→ PoC 2 (Graph+Vec)
                                                         ↕
           PoC 3 (QuickJS) ─── PoC 5 (MCP) ──────────── PoC 4 (Embedded)
```

1. **PoC 1 + PoC 6** first — they directly validate the application architecture (daemon + UI). PoC 6 can be combined with PoC 1 (make the Tauri+Solid app the daemon's client).
2. **PoC 2** next — the knowledge graph is the persistent backbone of the system.
3. **PoC 3, 4, 5** can be done in any order — they validate pluggable subsystems.

### PoC Directory Structure

```
poc/
├── daemon/                # PoC 1: Daemon + socket API
│   ├── Cargo.toml
│   └── src/main.rs
├── tauri-client/          # PoC 1 + 6: Tauri v2 + SolidJS client
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── src/               # Rust commands
│   └── ui/                # SolidJS + Vite + Tailwind
├── knowledge-graph/       # PoC 2: SQLite + FTS5 + sqlite-vec
│   ├── Cargo.toml
│   └── src/main.rs
├── quickjs-sandbox/       # PoC 3: rquickjs async interop
│   ├── Cargo.toml
│   ├── src/main.rs
│   └── test-scripts/      # JS test files
├── embedded-model/        # PoC 4: llama-cpp-rs inference
│   ├── Cargo.toml
│   └── src/main.rs
└── mcp-client/            # PoC 5: rmcp stdio + SSE
    ├── Cargo.toml
    └── src/main.rs
```

### PoC Success Summary

| PoC | Risk | Pass? | Proceed to Phase | Notes |
|-----|------|-------|------------------|-------|
| 1. Daemon-First | 🔴 | ✅ | Phase 0, 1 | axum + tokio; 3/3 tests pass |
| 2. Graph+Vec | 🔴 | ✅ | Phase 4 | FTS5 0.21ms, KNN 6.8ms/10K, CTE 10ms avg; `k=?` constraint required on vec0 queries |
| 3. QuickJS | 🔴 | ✅ | Phase 10 | rquickjs 0.11 async interop works; use `MutFn::from()` + `parking_lot::Mutex` for sync callbacks; 4/4 tests pass |
| 4. Embedded Models | 🟡 | ✅ | Phase 5 | llama-cpp-2 v0.1.138 compiles on Windows; CPU inference ready; no external CMake needed |
| 5. MCP Client | 🟡 | ✅ | Phase 6 | rmcp compiles; `tool.description` is `Cow<str>` not `Option`; 1/1 tests pass |
| 6. Tauri+Solid | 🟡 | ✅ | Phase 0, 16 | Tauri v2 + SolidJS + Vite + TS scaffolded; frontend builds in 2.6s; Rust backend compiles clean |

> **All 6 PoCs passed.** Phase 0 is unblocked. Key learnings have been folded into notes above.

---

## Phase 0 — Project Scaffolding

**Goal:** Empty but buildable workspace. CI green. Dev tooling in place.

### 0.1 Workspace Init
- Create Cargo workspace with all crate stubs (lib.rs / main.rs with `todo!()` or empty impls)
- Choose and configure error handling (`thiserror` + `anyhow`)
- Set up `tracing` crate for structured logging across all crates
- Configure `clippy` lints and `rustfmt` settings

### 0.2 Tauri App Init
- Scaffold from PoC 6 result (Tauri v2 + SolidJS + Vite + Tailwind)
- Verify build on macOS + Windows + Linux
- Set up hot-reload dev workflow

### 0.3 CI/CD
- GitHub Actions: build + test + clippy + fmt on macOS + Windows + Linux
- Tauri build pipeline (produces unsigned installers for all three platforms)
- Dependabot / Renovate for dependency updates
- Add code coverage reporting

### 0.4 Test Infrastructure (`hive-test-utils`)
- `MockProvider`: pattern-matched canned responses, streaming simulation, failure injection, call capture
- `RecordedProvider`: record/replay for real API sessions — strip secrets, commit fixtures
- `MockMcpServer`: in-process MCP server returning scripted tool results
- `MockMessagingBridge`: fake Discord/Slack/Telegram adapter for bridge tests
- `TestDaemon`: helper to spin up an in-process daemon with mock providers for integration tests
- **Rule:** `cargo test --workspace` passes with zero API keys or network access

### 0.5 Developer Docs
- CONTRIBUTING.md with setup instructions
- Architecture diagram (from this plan)
- Crate dependency graph

**Milestone:** `cargo build --workspace` and `npm run tauri dev` both succeed on macOS + Windows + Linux.

---

## Phase 1 — Core Daemon & Infrastructure

**Goal:** Long-lived background daemon with config loading, credential storage, event bus, and audit logging. CLI can start/stop/status the daemon.

*Spec refs: §3.1–3.3, §5.5 (Audit Log, Credential Vault), §7.3 (Event Bus), §14.1*

### 1.1 Config System (`hive-core`)
- Define `HiveMindConfig` struct with serde deserialization
- Load from `~/.hivemind/config.yaml` with project-level `.hivemind/config.yaml` overlay
- Schema validation with helpful error messages
- Config hot-reload (watch file, emit event on change)

### 1.2 Credential Vault (`hive-crypto`)
- macOS Keychain integration via `security-framework` crate
- Windows Credential Manager via `windows-credentials` crate
- Trait: `CredentialStore { get, set, delete }` with platform impls
- `env:VAR_NAME` resolver for config values that reference env vars

### 1.3 Audit Logger (`hive-core`)
- Append-only structured log (JSON lines or SQLite table)
- Tamper-evident: each entry includes a chained hash of the previous entry
- Log schema: `{ id, timestamp, actor, action, subject, data_class, detail_hash, outcome }`
- Retention and rotation policy

### 1.4 Event Bus (`hive-core`)
- In-process async pub/sub using `tokio::broadcast` / `tokio::sync::watch`
- Topic-based routing (string topics with wildcard subscriptions)
- Event envelope: `{ topic, payload, timestamp, source }`
- Back-pressure: bounded channels with configurable overflow policy (drop oldest / block)

### 1.5 Daemon Process (`hive-daemon`)
- Tokio async runtime
- Platform-specific daemonisation:
  - macOS: launchd plist generation + install command
  - Windows: register as Windows Service (via `windows-service` crate) or Task Scheduler
- Graceful shutdown: drain active tasks, checkpoint state, close connections
- PID file / socket lock to prevent multiple instances
- Health check endpoint

### 1.6 Local API Server (`hive-api`)
- Unix socket (macOS) / named pipe (Windows) for local IPC
- HTTP + WebSocket server on localhost (configurable port, disabled by default)
- JSON-RPC or REST API — define initial endpoints:
  - `daemon/status`, `daemon/shutdown`
  - `config/get`, `config/reload`
- Authentication: token-based for HTTP (token auto-generated, stored in config dir)

### 1.7 CLI Foundation (`hive-cli`)
- `clap`-based CLI with subcommands
- `hive daemon start` / `stop` / `status`
- `hive config show` / `hive config validate`
- Connects to daemon via local socket; starts daemon if needed

**Milestone:** `hive daemon start` runs a persistent background process. `hive daemon status` reports it's alive. `hive daemon stop` shuts it down cleanly. Config file is loaded and validated.

---

## Phase 2 — Data Classification & Security

**Goal:** Every piece of data flowing through HiveMind OS carries a classification label. A classification gate prevents data from crossing channel boundaries.

*Spec refs: §5.1–5.7*

### 2.1 Classification Types (`hive-classification`)
- Define `DataClass` enum: `PUBLIC(0)`, `INTERNAL(1)`, `CONFIDENTIAL(2)`, `RESTRICTED(3)`
- Define `ChannelClass` enum: `Public`, `Internal`, `Private`, `LocalOnly`
- Define `ClassificationLabel` struct: `{ level: DataClass, source: LabelSource, reason: Option<String>, timestamp }`
- Implement comparison and compatibility: `data_class_allowed_on_channel(data: DataClass, channel: ChannelClass) -> bool`

### 2.2 Automatic Labellers (`hive-classification`)
- **Pattern-based labeller:** regex rules for secrets, API keys, SSNs, emails, credit cards, etc.
  - Ship default rules; user can add custom patterns in config
  - Return highest-matching classification
- **Source-based labeller:** classify based on where data came from (file path, MCP server, clipboard source)
- **Labeller pipeline:** run all labellers, take `max(results)` as the effective label
- Extensible: `trait Labeller { fn classify(&self, content: &str, context: &LabelContext) -> Option<DataClass> }`

### 2.3 Classification Gate (`hive-classification`)
- Middleware function: `gate(data: &Classified, channel: &Channel) -> GateDecision`
- `GateDecision`: `Allow | Block { reason } | Prompt { context } | RedactAndSend { redactions }`
- Integrates with override policy configuration (§5.3)
- All decisions logged to audit log

### 2.4 Override Policy Engine (`hive-classification`)
- Load override policy from config (`security.override_policy`)
- Decision cache: in-memory LRU with TTL, keyed on `(data_hash, channel_id)`
- Rate limiting: `max_overrides_per_hour` counter
- Organisational lockdown: `managed: true` prevents weakening policies
- API endpoint for UI to submit user decisions (allow/deny/redact/reclassify)

### 2.5 Prompt Sanitiser (`hive-classification`)
- Given content + list of sensitive spans, produce redacted version with `[REDACTED]` placeholders
- Generate a diff showing what was removed
- Reversible: store original content locally (encrypted) so it can be "un-redacted" for audit

### 2.6 Policy Engine (`hive-classification`)
- OPA-style rule evaluation for classification decisions
- Rules defined in config; evaluated against context (data class, channel, user, agent role)
- Default policy: block RESTRICTED from all non-local channels; prompt for CONFIDENTIAL on public

**Milestone:** Strings can be classified. A gate function blocks/allows data flow to channels. Override prompts can be issued and resolved. Inbound data can be scanned for prompt injection. All decisions and scans are audited and queryable.

### 2.7 Prompt Injection Scanner (`hive-classification`)
- Isolated scanner model — separate LLM context with no tool access, no conversation history
- Scanner configuration: `security.prompt_injection` block (enabled, sources, action, threshold)
- `ScanVerdict` struct: payload_hash, source, risk level, confidence, threat_type, flagged_spans, recommendation
- Enforcement gate: block / prompt / flag / allow (mirrors classification override UX)
- Async scanning pipeline: scans run in parallel with agent thinking; results awaited before payload consumption
- Content-hash caching to avoid re-scanning identical payloads (`cache_ttl` configurable)
- Batching of small payloads from same loop iteration
- Skip rules for known-safe sources (configurable via `scan_sources`)
- Scanner model role (`scanner` in §4.4): prefer local/embedded model; falls back to admin model
- *Spec ref: §5.6*

### 2.8 Risk Scan Ledger (`hive-classification`)
- `risk_scans` table: id, scan_type, payload_hash, payload_preview, source, verdict, confidence, threat_type, flagged_spans, action_taken, user_decision, model_used, scan_duration_ms, data_class, scanned_at, session_id
- Indexes on scan_type, verdict, source, scanned_at, payload_hash
- Extensible scan types: `prompt_injection`, `classification`, `pii_detection`, `secret_detection`, custom
- All security scans (classification, injection, PII, secrets) write to the same ledger
- Query APIs for risk visibility: by type, by source, by date range, by verdict, by payload hash
- UI integration: "Risk Scans" tab in Audit Log view, shield icon in chat, dashboard widget
- *Spec ref: §5.7*

---

## Phase 3 — Multi-Provider Model Layer

**Goal:** Connect to multiple LLM providers. Route requests based on classification, preferences, and availability.

*Spec refs: §4.1–4.4*

### 3.1 Provider Trait (`hive-providers`)
- Define `ModelProvider` trait:
  ```
  async fn complete(request: CompletionRequest) -> Stream<CompletionChunk>
  async fn list_models() -> Vec<ModelInfo>
  fn channel_class() -> ChannelClass
  fn capabilities() -> ProviderCapabilities  // chat, code, vision, embedding, tool_use
  ```
- `CompletionRequest`: messages, model, temperature, tools, response_format, max_tokens
- `CompletionChunk`: text delta, tool call delta, usage stats
- Unified error type with retry hints

### 3.2 Provider Adapters (`hive-providers`)
- **OpenAI-compatible** — covers OpenAI, Ollama, OpenRouter, and any compatible API
- **Anthropic** — native Anthropic API with extended thinking, caching, etc.
- **Azure OpenAI** — Azure-specific auth (API key + Entra ID), deployment-based routing
- **Microsoft Foundry** — Foundry-specific API, model discovery via `/models`, Entra ID auth
- **GitHub Copilot** — GitHub OAuth / device flow, Copilot chat API, Extensions support
- Each adapter:
  - Handles auth (API key, OAuth, Entra ID)
  - Implements streaming
  - Handles rate limiting (429 + Retry-After) and transient errors
  - Exposes provider-specific options (OpenRouter `route`, Azure `api_version`, etc.)

### 3.3 Model Router (`hive-providers`)
- Input: `RoutingRequest { messages, required_capabilities, data_class, preferred_model, cost_budget }`
- Algorithm:
  1. Filter providers by `data_class ≤ channel_class`
  2. Filter by required capabilities (tool_use, vision, etc.)
  3. If user pinned a model, use it (if it passes classification)
  4. Score remaining by: user preference > cost > latency > availability
  5. Build fallback chain from eligible candidates
- Emit routing decision to audit log

### 3.4 Model Roles (`hive-providers`)
- Load `model_roles` config: `primary`, `admin`, `coding`, `vision`, custom roles
- Role resolution: explicit role → `admin` → `primary`
- Internal subsystems (classifiers, summarisers, routing) request models by role name
- Per-conversation / per-agent overrides

### 3.5 Streaming Infrastructure (`hive-core`)
- `AsyncIterator<CompletionChunk>` trait used across all providers
- Tool-call streaming: assemble partial tool-call JSON across chunks
- Structured output support: JSON mode, function calling
- Token usage tracking per-request (emit to event bus for cost tracking)

**Milestone:** `hive chat` sends a message through the model router, streams a response. Switching providers via config works. Classification blocks prompts with private data from reaching public providers.

---

## Phase 4 — Knowledge Graph

**Goal:** Persistent, queryable, classification-aware knowledge graph. The agent can remember, recall, and forget.

*Spec refs: §8.1–8.6*

### 4.1 Storage Engine (`hive-knowledge`)
- **Decision: SQLite** with property-graph schema (simpler build, excellent Windows support, proven ecosystem, already used for workflows/scheduler/audit).
- **Vector search:** [`sqlite-vec`](https://github.com/asg017/sqlite-vec) extension — pure C, no external dependencies, stores vectors as blobs, KNN via virtual tables. Load at runtime via `db.load_extension("vec0")`.
- SQLite schema:
  - `nodes` table: `id, label, name, description, data_class, confidence, embedding BLOB, properties (JSON), created_at, updated_at, last_accessed_at, ttl`
  - `edges` table: `id, source_id, target_id, rel_type, weight, data_class, properties (JSON), created_at`
  - FTS5 virtual table on `nodes.name`, `nodes.description`, `nodes.content`
  - `vec_nodes` virtual table (sqlite-vec): `CREATE VIRTUAL TABLE vec_nodes USING vec0(embedding float[384])` — dimension matches the embedding model (e.g., all-MiniLM-L6-v2 = 384d)
- Encryption at rest: SQLCipher or application-level AES-256 encryption

### 4.2 Node & Edge Types (`hive-knowledge`)
- Implement all node types from §8.2: Entity, Concept, Artifact, Event, Task, Observation, Preference, Conversation, Skill, Tool
- Implement all edge types from §8.2: RELATES_TO, IS_A, PART_OF, CREATED_BY, OBSERVED_IN, DEPENDS_ON, SUPERSEDES, DERIVED_FROM, USES_TOOL, MENTIONED_IN, TRIGGERED_BY, PREFERS, KNOWS_ABOUT, SIMILAR_TO
- Typed wrappers over the base schema with validation

### 4.3 Classification Propagation (`hive-knowledge`)
- `effective_class(node)` computation: `max(node.data_class, max(ancestors))`
- Efficient: cache effective class, invalidate on graph mutations
- Graph-level enforcement: queries always filter by classification ceiling

### 4.4 Memory Manager (`hive-knowledge`)
- Implement `MemoryManager` API from §8.4:
  - Write: `remember`, `learn_skill`, `track_entity`, `record_event`, `set_preference`, `link`, `supersede`, `forget`
  - Read: `recall`, `query_graph`, `get_context`, `similar`, `get_observations`
  - Maintenance: `decay`, `consolidate`, `reindex_embeddings`, `export`
- Expose as tool definitions for the agentic loop
- Namespace/scope support for multi-agent blackboards (§10.3)

### 4.5 Query Engine (`hive-knowledge`)
- **Keyword/FTS** — SQLite FTS5 with ranking
- **Graph traversal** — BFS/DFS from a node, with depth limit and classification ceiling
- **Semantic similarity** — `sqlite-vec` KNN queries: `SELECT rowid, distance FROM vec_nodes WHERE embedding MATCH ? ORDER BY distance LIMIT ?` (requires embedding model from Phase 5)
- **Hybrid** — combine FTS5 scores + `sqlite-vec` distances + graph neighbourhood traversal, re-rank by weighted relevance + recency + confidence
- All queries enforce `class_ceiling` parameter at the engine level

### 4.6 Memory Lifecycle (`hive-knowledge`)
- **Short-term buffer:** in-memory per-conversation store
- **Consolidation:** end-of-conversation merge into long-term graph (deduplicate, resolve contradictions)
- **Decay:** scheduled job reduces confidence of unaccessed nodes
- **Forget:** soft-delete with audit trail (record deletion event, not content)

**Milestone:** Agents can `remember("Alice prefers Rust")` and `recall("Alice's language preferences")`. Graph is persisted, encrypted, and classification-aware. FTS and graph traversal queries work.

---

## Phase 5 — Embedded Models

**Goal:** Run small models in-process for classification, embeddings, NL parsing, and other admin tasks.

*Spec refs: §4.5*

### 5.1 Inference Runtime (`hive-embedded-models`)
- Integrate `llama-cpp-rs` (bindings to llama.cpp) for GGUF model inference
- Alternative: `candle` (pure Rust, no C++ dependency, but less model format support)
- **Recommendation:** Start with `llama-cpp-rs` for broader model compatibility
- Support both CPU and GPU (Metal on macOS, CUDA/Vulkan on Windows)

### 5.2 Model Management (`hive-embedded-models`)
- Model registry: catalog of available models with metadata (size, quant levels, capabilities)
- Download manager: fetch GGUF/safetensors from HuggingFace Hub
- Storage: `~/.hivemind/models/` with manifest tracking installed models
- CLI: `hive model list --embedded`, `install`, `uninstall`, `status`

### 5.3 Memory Management (`hive-embedded-models`)
- Configurable memory ceiling (`embedded_models.max_memory_mb`)
- LRU eviction when at ceiling
- GPU layer offloading configuration (`auto`, explicit layer count)
- Preload list: models loaded at daemon start
- Lazy loading: models loaded on first use, evicted under pressure

### 5.4 Provider Integration (`hive-embedded-models`)
- Implement `ModelProvider` trait for embedded models
- `channel_class: LocalOnly` — data never leaves the process
- Register in provider registry as `embedded` provider type
- Wire into model roles: `admin` role can target embedded models

### 5.5 Embedding Support (`hive-embedded-models`)
- Load embedding models (all-MiniLM-L6-v2, GTE-small, etc.)
- Expose `embed(text: &str) -> Vec<f32>` API
- Wire into knowledge graph for vector indexing (§4.5 of this plan)
- Batch embedding for bulk KG operations

**Milestone:** `hive model install smollm2-360m` downloads a model. Embedded model can generate completions and embeddings. Admin model role uses embedded model for classification and summarisation.

---

## Phase 6 — MCP Integration

**Goal:** Full MCP client that discovers, connects to, and invokes MCP servers. Classification rules apply.

*Spec refs: §6.1–6.4*

### 6.1 MCP Client (`hive-mcp`)
- Implement MCP client protocol (JSON-RPC over stdio, SSE, Streamable HTTP)
- Capability negotiation
- Tool, resource, and prompt discovery
- Connection lifecycle: start, health-check, reconnect, stop

### 6.2 Server Registry & Lifecycle (`hive-mcp`)
- Load server configs from `mcp_servers` in config
- Process management for stdio servers (start/stop child processes)
- Connection management for HTTP/SSE servers
- Channel classification per server
- Server health monitoring and auto-restart

### 6.3 Notifications API (`hive-mcp`)
- Server → Client: tool list changes, resource updates, progress, log messages
- Client → Server: roots changed, cancellation, init confirmation
- Route notifications into the event bus (§1.4)
- Trigger scheduler events from notifications

### 6.4 Sampling Support (`hive-mcp`)
- Handle sampling requests from MCP servers
- Route through model router (inherit server's channel_class)
- User approval flow (configurable: always-ask, auto for trusted, deny)

### 6.5 Tool Integration (`hive-mcp`)
- Wrap MCP tools as `ToolDefinition` for the agentic loop
- Classification gate on tool inputs/outputs
- Tool policy enforcement (auto-approve, require confirmation, deny)
- Annotations support (readOnlyHint, destructiveHint, etc.)

**Milestone:** `hive` connects to configured MCP servers, discovers tools, and the agentic loop can invoke them. Notifications flow into the event bus. Classification prevents data leakage via MCP.

---

## Phase 7 — Agentic Loop (Core)

**Goal:** Pluggable reasoning engine. ReAct loop works end-to-end with model calls, tool use, and memory.

*Spec refs: §9.1–9.4*

### 7.1 Loop Architecture (`hive-loop`)
- Define `LoopStrategy` trait:
  ```
  async fn run(context: LoopContext) -> LoopResult
  ```
- Define `LoopContext`: session, conversation, history, tools, memory, data_class, metadata
- Define `LoopMiddleware` trait: `before_model_call`, `after_model_response`, `before_tool_call`, `after_tool_result`, `on_complete`
- Pipeline executor: runs middleware chain around each step

### 7.2 Built-in Strategies (`hive-loop`)
- **ReAct** — Reason → Act → Observe cycle (implement first, this is the baseline)
- **Plan-and-Execute** — generate plan, execute steps
- **Reflexion** — ReAct + self-critique after each action
- **Tree-of-Thought** — explore branches, select best (can defer)
- **Human-in-the-Loop** — pause at checkpoints for user confirmation

### 7.3 Core Middleware (`hive-loop`)
- **Classification Gate** — check data labels before every model/tool call
- **Memory Augmentation** — inject relevant KG context into prompts
- **Cost Tracker** — track tokens, enforce budgets
- **Audit Logger** — log every action

### 7.4 Tool Executor (`hive-loop`)
- Resolve tool calls to built-in tools or MCP tools
- Apply tool policy (auto-approve / confirm / deny)
- Execute with timeout
- Return results through middleware chain

### 7.5 Loop Configuration (`hive-loop`)
- Load from `agent_loop` config section
- Strategy selection, max iterations, model selection
- Middleware ordering
- Tool policy
- Memory settings (auto_observe, context_window, class_ceiling)
- Fallback behaviour (retry with reflection, escalate, try alt model)

**Milestone:** User sends a message via CLI → ReAct loop reasons, calls tools, queries memory, and returns a response. Middleware pipeline fires correctly. Classification gate blocks private data from public models.

---

## Phase 8 — Built-in Tools

**Goal:** Ship a useful set of built-in tools that the agentic loop can invoke.

*Spec refs: §11.1, §11.4*

### 8.1 Tool Framework (`hive-tools`)
- Define `Tool` trait: `async fn execute(input: Value) -> ToolResult`
- Tool registry: discover and register tools at startup
- Tool metadata: `ToolDefinition` with schema, classification, side-effects, approval level
- Tool sandboxing: configurable approval policy per-tool

### 8.2 Filesystem Tools (`hive-tools`)
- `read`, `write`, `list`, `search` (ripgrep-style), `glob`, `diff`, `patch`
- Respect classification: files in certain paths auto-classified
- Configurable allowed directories (sandbox)

### 8.3 Shell Tools (`hive-tools`)
- Execute commands in a sandboxed shell
- Approval policy: deny by default, require confirmation
- Timeout and output limits
- Platform-specific (PowerShell on Windows, bash/zsh on macOS)

### 8.4 Web Tools (`hive-tools`)
- `fetch` — download URL content (HTML → markdown)
- `search` — web search via configured provider
- `screenshot` — capture webpage screenshot (headless browser)

### 8.5 Code Tools (`hive-tools`)
- `lint` — run configured linters
- `test` — run test suite
- `build` — run build command
- Syntax-aware edit / refactor (tree-sitter based)

### 8.6 Data Tools (`hive-tools`)
- SQL query (SQLite/DuckDB)
- CSV/JSON transform
- Regex extraction

### 8.7 System Tools (`hive-tools`)
- Clipboard read/write
- Desktop screenshot
- Window listing

### 8.8 Knowledge Graph Tools (`hive-tools`)
- Expose all MemoryManager operations as callable tools
- `/remember`, `/recall`, `/forget`, `/classify` commands

**Milestone:** The agentic loop can read files, run shell commands (with approval), search the web, and query the knowledge graph — all as tool calls within a conversation.

---

## Phase 9 — Background Task Scheduler

**Goal:** Cron, interval, event-driven, and one-shot task scheduling.

*Spec refs: §7.1–7.4*

### 9.1 Task Model (`hive-scheduler`)
- Define `Task` struct from §7.1
- Task persistence in SQLite
- Task states: pending → running → completed/failed, with pause/cancel

### 9.2 Schedule Engine (`hive-scheduler`)
- Cron parser (use `cron` crate)
- Interval scheduling
- One-shot (deferred execution)
- Event-triggered: subscribe to event bus topics, fire task on match

### 9.3 Task Executor (`hive-scheduler`)
- Each task runs an agentic loop with its own config
- Concurrency limits (configurable `max_concurrent_tasks`)
- Per-provider rate limiting (token bucket)
- Task inherits creator's data classification context
- Timeout enforcement

### 9.4 Task Management (`hive-scheduler`)
- CLI: `hive task create`, `list`, `pause`, `resume`, `cancel`, `logs`
- API endpoints for UI

**Milestone:** `hive task create --cron "0 9 * * MON-FRI" --role researcher --loop deep-research --param query="weekly AI news"` creates a recurring task. Tasks run on schedule and produce results.

---

## Phase 10 — Workflow Engine & Loop DSL

**Goal:** YAML-based loop authoring with durable execution. Loops survive restarts.

*Spec refs: §9.5–9.8*

### 10.1 DSL Parser (`hive-loop`)
- Parse `.loop.yaml` files into a `LoopDefinition` AST
- Validate: schema checking, stage graph connectivity, type checking for state references
- Template engine for `{{state.x}}` expressions (Handlebars-style)

### 10.2 DSL Stage Types (`hive-loop`)
- Implement all stage types from §9.5:
  - `model_call`, `tool_call`, `parallel_tool_calls`
  - `conditional`, `loop`, `human_input`
  - `memory_read`, `memory_write`
  - `checkpoint`, `sub_loop`
  - `custom_stage`, `terminal`

### 10.3 Custom Stage Sandboxing (`hive-loop`)
- Embed QuickJS (via `rquickjs` crate) or V8 isolate (via `rusty_v8` / `deno_core`)
- **Recommendation:** QuickJS — much smaller binary, simpler build, sufficient for custom stages
- Implement `@hivemind/loop-sdk` API surface:
  - `ctx.tools.call()` — route through tool executor (classification applies)
  - `ctx.model.complete()` — route through model router (classification applies)
  - `ctx.state` — read/write workflow state
- Sandbox constraints: no fs, no network, CPU timeout, memory limit

### 10.4 Event-Sourced State Machine (`hive-workflow`)
- Append-only event log in SQLite (`workflow_events` table)
- State snapshots via JSON Patch deltas
- Periodic checkpointing (`workflow_checkpoints` table)
- Replay-based recovery on restart

### 10.5 Workflow Persistence (`hive-workflow`)
- `workflow_runs` table: id, loop_id, status, state, params, data_class
- `workflow_events` table: run_id, sequence, stage, event_type, payload, state_patch
- `workflow_checkpoints` table: run_id, event_sequence, state_snapshot
- All data encrypted at rest

### 10.6 Durable Execution (`hive-workflow`)
- On restart: scan for incomplete runs, replay from last checkpoint
- Idempotency keys for retried tool calls
- Saga/compensation: define compensating actions for rollback
- Nested workflows: sub-loops as child runs

### 10.7 Resumable Execution (`hive-workflow`)
- `human_input` stages: pause, persist, resume on user response
- Manual pause/resume from CLI and UI
- Timeout-based pause (checkpoint and suspend after configurable duration)

**Milestone:** A `deep-research.loop.yaml` runs end-to-end. If the daemon restarts mid-run, it resumes from the last checkpoint. Custom TypeScript stages execute in the sandbox.

### 10.8 Context Compaction (`hive-loop`, `hive-graph`)
- Implement `ContextCompactor` middleware (§9.12):
  - Token estimation for conversation history
  - Trigger at configurable threshold (default 75%)
  - Extract entities/observations/decisions into KG nodes with embeddings
  - Generate prose summaries via admin model
  - Prune old turns from `LoopContext.history`
- Recursive compaction: oldest summaries roll up into epoch summaries
- Reconstruction: vector + FTS5 + graph traversal to recall compacted knowledge
- User commands: `/compact`, `/recall <query>`, `/history`
- Emit `context_compacted` and `memory_recalled` loop events
- **Depends on:** Phase 4 (Knowledge Graph), Phase 7 (Agentic Loop core)

**Milestone:** An agent session runs 200+ turns without context overflow. Compacted facts are recallable via `/recall`. Cross-session knowledge works (facts from session A surface in session B).

### 10.9 Session Forking (`hive-loop`, `hive-daemon`)
- Implement `SessionFork` schema and copy-on-write event referencing (§9.13)
- Fork types: head, historical, conversation
- State reconstruction across fork chains (parent checkpoint → parent events → fork events)
- Fork tree tracking and ancestor walking
- KG integration: read-only parent node visibility, copy-on-write cloning on mutation
- Data classification inheritance across fork boundaries
- REST API: `POST /sessions/{id}/fork`, `GET /sessions/{id}/forks`, `GET /sessions/{id}/ancestors`
- User commands: `/fork`, `/fork @N`, `/forks`, `/switch`
- UI: fork tree visualization (branch graph)
- **Depends on:** Phase 7 (Agentic Loop core), Phase 4 (Knowledge Graph)

**Milestone:** User can fork a 50-turn session at any point, both branches continue independently. KG queries in the fork see parent nodes. Classification propagation is correct across fork boundaries.

---

## Phase 11 — Loop Registry & Sigstore

**Goal:** Community loop discovery, installation, and supply-chain security.

*Spec refs: §9.9–9.10*

### 11.1 Registry Client (`hive-loop`)
- Search loops by keyword, author, tag
- Inspect: display manifest, permissions, signature status
- Download and extract loop packages
- Version resolution and update checking

### 11.2 Loop Package Format (`hive-loop`)
- `hive-loop.json` manifest parsing and validation
- Permission declarations: tools, model_calls, human_input, network, filesystem
- Dependency on `hive_sdk` version

### 11.3 Sigstore Integration (`hive-crypto`)
- Keyless signing via Sigstore (OIDC → ephemeral certificate → sign → Rekor log)
- Verification: check signature bundle → verify certificate identity → check Rekor
- Trust policy configuration: trusted authors/orgs, require signature, require transparency log
- Offline verification with cached trust root
- Use `sigstore-rs` crate

### 11.4 CLI Commands (`hive-cli`)
- `hive loop search`, `inspect`, `install`, `list`, `run`, `publish`
- Display signature verification results
- Warning for unsigned / untrusted loops

**Milestone:** `hive loop install hivemind-community/deep-research@1.2.0` verifies the Sigstore signature and installs the loop. `hive loop publish` signs and uploads.

---

## Phase 11.5 — Visual Loop Designer

**Goal:** A drag-and-drop canvas for building, debugging, and sharing agentic loops — backed by the same `.loop.yaml` format.

*Spec refs: §9.14*

### 11.5.1 Canvas Engine (`hive-ui`)
- Node-and-edge graph rendering (SolidJS + HTML5 Canvas or SVG)
- Stage palette: draggable stage types matching DSL primitives (§9.5)
- Edge drawing: click source port → click target port to create transitions
- Conditional stages: labelled edges per branch
- Auto-layout engine (dagre or elkjs) for imported YAML
- Canvas controls: pan, zoom, undo/redo stack, selection, multi-select, delete
- Layout persistence in `.loop.layout.json` (positions separate from YAML)

### 11.5.2 YAML Round-Tripping (`hive-loop`)
- Parse `.loop.yaml` → canvas graph model (lossless)
- Serialize canvas graph model → `.loop.yaml` (preserving comments via YAML AST)
- Real-time structural validation: unreachable nodes, missing transitions, type mismatches
- Live error badges on invalid nodes/edges

### 11.5.3 Properties Panel (`hive-ui`)
- Dynamic form generation from stage type schema
- Prompt template editor with `{{state.*}}` autocomplete
- Tool selector (from registered tools/MCP)
- Condition builder with state path autocomplete
- Error handler configuration
- Security constraints editor (max_data_class, allowed_channels)

### 11.5.4 Template Library (`hive-ui`, `hive-loop`)
- Bundled templates: Simple Chat, ReAct, Deep Research, Code Assistant, RAG Pipeline, Code Review, Data Pipeline, Creative Writing, Multi-Agent Coordinator, Conversational
- Browse community templates from Loop Registry (§9.9)
- Template preview: read-only canvas rendering before loading
- "Start from template" → loads onto canvas for customisation
- Signature verification for community templates (§9.10)

### 11.5.5 Live Preview & Debugging (`hive-ui`, `hive-loop`)
- **Test mode**: run loop with test inputs, highlight active stage on canvas
- **Replay mode**: load a past workflow run from event log (§9.7) and replay on canvas
- **Breakpoints**: click stage edge to set; execution pauses, showing state inspector
- **Dry run**: mocked model/tool responses for cost-free control flow testing
- Per-stage execution time, state diff, classification level display

### 11.5.6 Custom Stage Editor (`hive-ui`)
- Embedded Monaco editor for TypeScript handlers (§9.6)
- Autocomplete for HiveMind OS sandbox API types
- "Test this stage" — isolated execution with sample input
- Inline type checking against declared input/output schema

### 11.5.7 Classification Overlay (`hive-ui`)
- Toggle colour-coded classification overlay (green/blue/amber/red per stage)
- Warning icons on edges crossing classification boundaries
- Security panel for editing `.loop.yaml` security section visually

**Depends on:** Phase 16 (Desktop UI), Phase 10 (Agentic Loops / Workflow DSL), Phase 11 (Loop Registry)

**Milestone:** User opens designer, picks "Deep Research" template, adds a custom stage, sets a breakpoint, runs test mode, sees execution flow on canvas. Saves as `.loop.yaml`, reopens in text editor — YAML is clean and complete. Reopens in designer — layout is preserved.

---

## Phase 12 — Roles & Multi-Agent System

**Goal:** User-defined agent personas. Multiple agents running simultaneously, communicating and collaborating.

*Spec refs: §10.1–10.7*

### 12.1 Role Definitions (`hive-agents`)
- Parse `.role.yaml` files
- Built-in role templates: assistant, researcher, developer, reviewer, architect, ops, scribe
- Role inheritance (`extends:` field)
- Role validation: check tool references, model references, KG scopes

### 12.2 Agent Instance Lifecycle (`hive-agents`)
- Spawn agent instances from roles (conversation, background, daemon modes)
- Instance state machine: Active → Working → Paused → Terminated
- Instance persistence: conversation history, workflow state
- Instance limits (configurable max concurrent per role)

### 12.3 Inter-Agent Communication (`hive-agents`)
- **Direct messaging:** point-to-point `AgentMessage` with typed message types
- **Pub/sub channels:** topic-based broadcast with persistent history
- **Shared blackboard:** KG namespace access for collaborative agents
- **Task delegation:** supervised child agents with supervision levels (autonomous, check_in, approval_gates, pair)

### 12.4 Inter-Agent Security (`hive-agents`)
- Message classification enforcement (sender can't exceed recipient's ceiling)
- Channel classification on pub/sub channels
- Role visibility restrictions
- Delegation ceiling (min of parent and child)
- Blackboard scope enforcement
- All inter-agent messages audited

### 12.5 Coordination Patterns (`hive-agents`)
- DSL stage types: `agent_spawn`, `parallel_agent_spawn`
- Built-in patterns: pipeline, debate/adversarial, swarm, supervisor/worker
- Examples as installable loops

### 12.6 Agent Dashboard API (`hive-api`)
- List all running agents with status, role, task, messages
- Agent detail: conversation history, workflow state, message log
- Start/stop/pause/resume agents
- Send messages to agents

**Milestone:** User defines a `code-reviewer` role. Spawns a reviewer agent that communicates with a developer agent. The architect agent delegates tasks and receives results.

---

## Phase 13 — Agent Skills

**Goal:** Support the Agent Skills open standard for portable procedural knowledge.

*Spec refs: §11.3*

### 13.1 Skill Loader (`hive-skills`)
- Scan configured directories for `SKILL.md` files
- Parse frontmatter: name, description, tags, allowed-tools, arguments
- Build lightweight in-memory skill index (name + description only)

### 13.2 Skill Activation (`hive-skills`)
- **Suggest mode:** match user task against skill descriptions (keyword + embedding similarity), suggest to user
- **Auto mode:** activate matching skills automatically within policy
- **Manual mode:** explicit `/skill activate <name>` only
- Load full `SKILL.md` body into agentic loop context on activation
- Lazy-load resources (`scripts/`, `references/`, `assets/`) on demand

### 13.3 Skill + Classification (`hive-skills`)
- Per-skill `data_class` labels
- Conversation ceiling update on activation: `max(conversation_class, skill_class)`
- Scripts run through custom stage sandbox (same as §10.3)

### 13.4 Skill + Roles (`hive-skills`)
- Role definitions declare allowed/denied skills
- Skills scoped per agent instance

### 13.5 CLI (`hive-cli`)
- `hive skill list`, `validate`, `activate`, `install`, `init`

**Milestone:** Agent auto-activates a `create-presentation` skill when user asks to make a presentation. Skill instructions guide the agent's behaviour. Classification rules apply.

---

## Phase 14 — HiveMind OS Peering

**Goal:** Trusted cross-machine connections with encrypted transport, agent messaging, and federated knowledge sync.

*Spec refs: §12.1–12.7*

### 14.1 Peer Identity (`hive-peering`)
- Generate long-lived Ed25519 key pair on first run
- Store in OS keychain
- Derive PeerID from public key
- Peer registry: `~/.hivemind/peers.yaml` (encrypted)

### 14.2 Pairing Flow (`hive-peering`)
- Generate short-lived pairing codes
- Rendezvous: LAN mDNS discovery or relay-assisted
- Mutual key exchange (Noise IK pattern)
- Verification fingerprint display
- Trust level assignment

### 14.3 Transport Layer (`hive-peering`)
- Noise Framework (IK pattern) over QUIC (use `quinn` + `snow` crates)
- Direct connections: LAN, Tailscale auto-detection
- Relay fallback: zero-knowledge relay server
- Offline queue: local message/sync queue for disconnected peers

### 14.4 Cross-Peer Communication (`hive-peering`)
- Extend agent messaging with peer routing (`to: { peer, role }`)
- Cross-peer delegation
- Classification enforcement: peer link has `data_class_ceiling`
- Exposed roles configuration

### 14.5 Federated Knowledge Sync (`hive-peering`)
- Scoped bidirectional sync (namespace-based)
- Sync configuration per peer per scope
- Conflict resolution strategies: last-write-wins, merge, manual, higher-confidence
- Classification enforcement during sync (exclude nodes above ceiling)
- Sync log for audit

### 14.6 Remote Capabilities (`hive-peering`)
- Shared model access (remote models appear as provider entries)
- Shared MCP servers (proxy access)
- Shared Agent Skills

### 14.7 CLI & Dashboard (`hive-cli`, `hive-api`)
- `hive peer invite`, `join`, `list`, `status`, `trust`, `revoke`, `disconnect`
- Peer dashboard API for UI

**Milestone:** Two HiveMind OS instances pair, exchange messages, sync a knowledge graph namespace, and one delegates a task to the other — all encrypted and classification-aware.

---

## Phase 15 — Mobile Communication Channels

**Goal:** Stay connected to agents from mobile via Discord, Slack, Telegram. Inspired by [air-traffic](https://github.com/danielgerlag/air-traffic).

*Spec refs: §3.1 (Messaging Bridges)*

### 15.1 Messaging Adapter Trait (`hive-messaging`)
- Define `MessagingAdapter` trait:
  ```
  async fn connect() -> Result<()>
  async fn send_message(channel: ChannelRef, content: RichMessage) -> Result<()>
  async fn on_message(handler: MessageHandler)
  async fn send_interactive(channel: ChannelRef, menu: InteractiveMenu) -> Result<UserChoice>
  async fn upload_file(channel: ChannelRef, file: FileAttachment) -> Result<()>
  fn channel_class() -> ChannelClass
  ```
- `RichMessage`: text, embeds, attachments, code blocks, images
- `InteractiveMenu`: buttons, selects, approval prompts (maps to Block Kit / Discord Components)

### 15.2 Discord Bridge (`hive-messaging`)
- Discord bot using `serenity` or `twilight` crate
- Dedicated channel per project / agent role
- Thread-based conversations (one thread = one agent session)
- Interactive components: buttons for approve/deny, dropdowns for role selection
- File upload/download
- Slash commands (`/hive ask`, `/hive task`, `/hive agent`, `/hive status`)

### 15.3 Slack Bridge (`hive-messaging`)
- Slack bot via Bolt SDK (or raw API)
- Block Kit for rich interactive messages
- Dedicated channels per project / agent role
- Thread-based conversations
- Slash commands
- Home tab with agent status dashboard

### 15.4 Telegram Bridge (`hive-messaging`)
- Telegram bot API via `teloxide` crate
- Inline keyboards for interactive prompts
- Group and private chat support
- File sharing

### 15.5 NL Intent Parser (`hive-messaging`)
- Use the admin model (§4.4) to parse natural language from messaging into structured commands
- Intent categories: ask question, run task, check status, approve/deny, manage agents
- Context-aware: reference previous messages in thread
- Falls back to forwarding raw text to the active agent session

### 15.6 Classification-Aware Messaging (`hive-messaging`)
- Each messaging platform gets a `channel_class` (typically `public` or `internal`)
- Classification gate applies to all outbound messages
- Sensitive content is redacted or blocked before sending
- Override prompts delivered *through the messaging platform itself* (approve/deny buttons)
- Inbound messages are classified on receipt

### 15.7 Session Management (`hive-messaging`)
- Start/join agent sessions from messaging
- Multiple concurrent sessions per user
- Session routing: messages in a thread route to the correct agent instance
- Background agents can proactively notify via messaging (task complete, alert, approval needed)

### 15.8 Web Console (`hive-api`)
- Browser-based UI at `localhost:PORT` (or exposed via Tailscale / reverse proxy)
- Shares frontend code with Tauri app
- Full functionality: chat, agents, tasks, knowledge, settings
- Mobile-responsive for phone browser access

### 15.9 Configuration
```yaml
messaging:
  discord:
    enabled: true
    bot_token: env:DISCORD_BOT_TOKEN
    guild_id: "123456789"
    channel_class: public            # Treat Discord as public channel
    channels:
      general: "channel-id-1"       # Map project channels
      alerts:  "channel-id-2"
    features:
      interactive_prompts: true      # Use buttons for approve/deny
      file_sharing: true
      thread_per_session: true

  slack:
    enabled: false
    bot_token: env:SLACK_BOT_TOKEN
    app_token: env:SLACK_APP_TOKEN
    channel_class: internal          # Org Slack = internal

  telegram:
    enabled: false
    bot_token: env:TELEGRAM_BOT_TOKEN
    channel_class: public
    allowed_users: [123456789]       # Telegram user IDs
```

**Milestone:** User receives a Discord DM when a background task completes. They reply "approve" and the agent resumes. Classification prevents sensitive data from leaking into the messaging channel.

---

## Phase 15.5 — Plugin System

**Goal:** Third-party extensibility via a well-defined plugin API.

*Spec refs: §14.2*

### 15.5.1 Plugin Manifest (`hive-plugin`)
- Define `hive-plugin.json` manifest schema (name, version, entry points, permissions)
- Plugin discovery: local directories, npm packages, Git repos

### 15.5.2 Extension Points (`hive-plugin`)
- **Model providers** — register custom provider adapters
- **Tools** — register additional tools (beyond MCP and built-in)
- **Middleware** — pre/post processing hooks for model calls and tool calls
- **Classifiers** — custom data-classification logic
- **UI panels** — custom views in the app (via webview extensions)

### 15.5.3 Plugin Lifecycle (`hive-daemon`)
- Install, enable, disable, uninstall
- Isolation: plugins run in their own QuickJS sandbox or as Rust dylibs
- Permission model: plugins declare required capabilities, user approves at install time
- Hot-reload for development mode

- **Depends on:** Phase 7 (Agentic Loop), Phase 8 (Tools)

**Milestone:** A third-party plugin adds a custom tool, a middleware hook, and a UI panel. It installs from an npm package and hot-reloads during development.

---

## Phase 16 — Tauri Desktop UI

**Goal:** Full-featured desktop application that connects to the daemon as a first-class client.

*Spec refs: §3.2, §13.1*

### 16.1 Tauri Shell (`tauri-app`)
- Tauri v2 with webview frontend
- Connects to daemon via local socket (same API as CLI and messaging bridges)
- Auto-start daemon on app launch if not running
- System tray icon with quick actions (new conversation, agent status, toggle)
- Global keyboard shortcut for launcher / quick input

### 16.2 Conversation View (`tauri-app`)
- Chat interface with streaming responses
- Tool call visualisation (collapsible blocks showing tool name, input, output)
- Classification badges on messages (colour-coded per data level)
- Code block rendering with syntax highlighting
- File preview for attachments and generated files
- Multi-conversation tabs or sidebar

### 16.3 Agent Dashboard (`tauri-app`)
- List all running agent instances with role, status, current task
- Spawn new agent from role template
- Agent detail: conversation history, workflow state, inter-agent messages
- Visual pipeline view for multi-agent coordination patterns
- Start/stop/pause/resume controls

### 16.4 Knowledge Graph Explorer (`tauri-app`)
- Visual graph view (force-directed or hierarchical)
- Node/edge detail panels
- Search: FTS, graph traversal, semantic similarity
- Classification overlay: colour nodes by data class
- Add/edit/delete nodes manually
- Namespace browser for multi-agent blackboards

### 16.5 Settings & Admin (`tauri-app`)
- Provider configuration (add/edit/test connections)
- MCP server management (add/remove, status, logs)
- Data classification rules editor
- Model roles configuration
- Embedded model management (install/uninstall, memory usage)
- Peer management (invite, trust levels, sync status)
- Messaging bridge configuration
- Task/scheduler management

### 16.6 Security UX (`tauri-app`)
- Classification override prompt modal (rich UI from §5.3)
- Tool approval dialogs with context
- Audit log viewer with filters
- Data flow visualisation (where data goes, what classification applies)

### 16.7 Accessibility & Platform (`tauri-app`)
- Full keyboard navigation
- Screen reader support
- Dark/light theme
- Platform-native notifications (macOS Notification Center, Windows Toast)
- Native file dialogs

**Milestone:** Desktop app launches, connects to daemon, supports full chat with tool use, agent management, knowledge graph browsing, and settings — all classification-aware.

### 16.8 First-Run Experience & Onboarding (`tauri-app`)
- Welcome screen with provider connection wizard (§13.4):
  - API key (auto-detect provider from key format)
  - GitHub OAuth (Copilot models)
  - OpenRouter OAuth/key
  - Local model picker (Small/Medium/Large, download with progress bar)
  - Advanced setup (full provider config form)
- Auto-detection of existing credentials (`~/.openai`, VS Code Copilot token, `~/.config/claude/`, env vars)
  - Import prompt as non-blocking toast
- Config migration from existing MCP configs (`mcp.json`, `claude_desktop_config.json`)
- Smart defaults bootstrapping: model roles auto-assignment, security defaults, KG initialisation
- Empty state design for all views (Chat, Agents, Tasks, Knowledge, Tools, Designer, Peers)
  - Each empty state has actionable prompts, not blank panels
- Contextual feature discovery tips (one per session, dismissable, never repeated)
- Optional guided tour (tooltip overlays, < 2 minutes, skippable, resumable)
- **Depends on:** Phase 16.1–16.7

**Milestone:** Fresh install → launch → pick "I have an API key" → paste key → land in a working chat with greeting and suggestions — total time under 60 seconds. No config file touched. All views have helpful empty states.

---

## Phase 17 — Testing, Security Audit & Hardening

**Goal:** Comprehensive test coverage, security review, and production readiness.

### 17.1 Unit Testing
- Target 80%+ coverage on core crates (`hive-classification`, `hive-knowledge`, `hive-loop`, `hive-workflow`)
- Property-based testing for classification propagation (use `proptest`)
- Fuzzing for input parsing (config, DSL, MCP messages)

### 17.2 Integration Testing
- End-to-end test harness: daemon + mock providers + test MCP servers
- Test scenarios:
  - Classification gate blocks private data from public channel
  - Override prompt flow (prompt → allow → audit)
  - Workflow checkpoint + restart + replay
  - Multi-agent communication patterns
  - Peering: pair → sync → delegate
  - Messaging bridge: receive message → parse intent → execute → respond

### 17.2.1 Adversarial E2E Scenario Suite (`e2e/scenarios/`)
- **Minimum 200 complex UI-driven scenarios** designed to break things (see TESTING_GUIDE.md §11)
- Scenarios defined in YAML, executed via Playwright + CDP against running HiveMind OS
- Categories: classification boundary (25), prompt injection (25), chat interaction (20), agentic loop stress (20), session forking (15), knowledge graph (15), multi-agent (15), MCP (15), model layer (15), peering (10), messaging bridges (10), visual loop designer (10), first-run & config (5)
- Three test modes: `mock` (every PR, ~10 min), `live` (nightly, ~60 min), `chaos` (weekly, fault injection)
- Chaos mode injects: API timeouts, rate limits, server disconnects, malformed JSON, network partitions, artificial latency
- Reports: JUnit XML, HTML with failure screenshots, risk coverage matrix, flaky test tracker
- **Gate rule:** every new SPEC feature requires ≥2 adversarial scenarios before considered complete
- 200 is a floor — suite grows with every bug fix and new feature

### 17.3 Security Audit
- Threat model review (STRIDE analysis)
- Cryptographic review: key management, Noise protocol usage, encryption at rest
- Sandbox escape analysis: QuickJS custom stage isolation
- Classification bypass analysis: all paths that touch outbound channels
- Dependency audit: `cargo audit`, `npm audit`, SBOM generation

### 17.4 Performance Testing
- Model routing latency benchmarks
- Knowledge graph query performance at scale (10K, 100K, 1M nodes)
- Workflow engine throughput (concurrent runs)
- Memory profiling for embedded models
- Event bus back-pressure under load

### 17.5 Hardening
- Rate limiting on all API endpoints
- Input validation and sanitisation everywhere
- Graceful degradation: daemon works without network, without GPU, with SQLite corruption recovery
- Crash reporting (optional, opt-in, classification-aware — no private data in crash reports)

**Milestone:** All tests pass on macOS + Windows + Linux. 200+ adversarial E2E scenarios pass in mock mode. Chaos mode runs weekly with no critical failures. Security review complete. Performance meets baseline targets. No known classification bypass.

---

## Phase 18 — Distribution & Packaging

**Goal:** Ship installable binaries for macOS and Windows. Auto-update. Documentation.

### 18.1 Platform Packages
- **macOS:** DMG installer, notarised and stapled. Homebrew cask formula.
  - launchd plist for daemon auto-start
  - Universal binary (aarch64 + x86_64)
- **Windows:** MSI installer (via WiX or Tauri bundler). Winget manifest.
  - Windows Service registration or Task Scheduler entry
  - Code signing (EV certificate)

### 18.2 Auto-Update
- Tauri's built-in updater (Sparkle on macOS, NSIS/WiX on Windows)
- Separate daemon updater (check on startup, notify, restart)
- Update channels: stable, beta, nightly
- Signature verification on update packages

### 18.3 Documentation
- User guide (getting started, configuration, CLI reference)
- Developer guide (architecture, contributing, crate docs)
- Loop authoring guide (DSL reference, custom stage API, publishing)
- Agent Skills integration guide
- API reference (local API for custom clients)

### 18.4 Telemetry (Optional, Opt-In)
- Anonymous usage statistics (features used, error rates — no content)
- Classification-aware: NEVER includes classified data
- Clear opt-in during first-run, toggleable in settings
- Open-source telemetry schema published

**Milestone:** Users can download, install, and start using HiveMind OS in under 5 minutes. Auto-update delivers new versions seamlessly.

---

## Dependency Graph

```
Phase 0: Scaffolding
  └─► Phase 1: Core Daemon
       ├─► Phase 2: Classification
       │    └─► Phase 3: Model Layer ────────────┐
       │         └─► Phase 5: Embedded Models     │
       ├─► Phase 4: Knowledge Graph ◄─────────────┘
       │    │   (needs embedding from Phase 5)
       │    └─► Phase 8.8: KG Tools
       ├─► Phase 6: MCP
       │    └─► Phase 8.5: MCP Tool wrappers
       └─► Phase 7: Agentic Loop (Core)
            ├─► Phase 8: Built-in Tools
            ├─► Phase 9: Background Scheduler
            │    └─► Phase 10: Workflow Engine & DSL
            │         ├─► Phase 10.8: Context Compaction
            │         ├─► Phase 10.9: Session Forking
            │         └─► Phase 11: Loop Registry & Sigstore
            ├─► Phase 12: Roles & Multi-Agent
            │    ├─► Phase 13: Agent Skills
            │    └─► Phase 14: HiveMind OS Peering
            ├─► Phase 15: Messaging Bridges
            └─► Phase 15.5: Plugin System

Phase 16: Tauri UI  ◄── (can start after Phase 1, iteratively adds features)
  ├─► Phase 16.8: First-Run Experience (after 16.1–16.7)
  └─► Phase 11.5: Visual Loop Designer (after Phase 16 + Phase 10 + Phase 11)
Phase 17: Testing   ◄── (continuous, gates each phase, full sweep before 18)
Phase 18: Distribution ◄── (after Phase 16 + 17)
```

---

## Milestones Summary

| Milestone | Phases | What You Can Do |
|-----------|--------|-----------------|
| **M1 — Foundation** | 0–1 | Daemon runs, config loads, CLI connects |
| **M2 — Secure Model Access** | 2–3 | Chat with LLMs, classification blocks private data from public providers |
| **M3 — Intelligent Memory** | 4–5 | Agent remembers & recalls, embedded models for admin tasks |
| **M4 — Connected Tools** | 6, 8 | MCP servers connected, rich toolbox available |
| **M5 — Agentic Core** | 7, 9–10, 10.8–10.9 | Full ReAct loop, scheduled tasks, durable workflows, YAML loop DSL, compaction, session forking |
| **M6 — Multi-Agent** | 11–13, 11.5 | Roles, inter-agent comms, skills, community loops, visual loop designer |
| **M7 — Networked** | 14–15 | Cross-machine peering, mobile messaging |
| **M8 — Ship It** | 16, 16.8, 17–18 | Desktop app with first-run onboarding, tested, packaged, distributed |

---

## Cross-Cutting Concerns

These apply across all phases and should be addressed continuously:

### Security
- Every outbound path must pass through the classification gate — no exceptions
- All secrets in OS keychain, never in config files or logs
- Audit log captures every significant action
- Sandbox for custom code (DSL stages, MCP tool results)

### Testing
- Each phase adds tests; CI gates merges
- Classification tests are the highest priority (any bypass is a P0 bug)
- Integration tests added as cross-crate features land

### Documentation
- Crate-level rustdoc for all public APIs
- User-facing docs updated as features ship
- Architecture decision records (ADRs) for significant choices

### Performance
- Streaming everywhere — no blocking on full LLM response
- Lazy loading: models, skills, KG data loaded on demand
- Connection pooling for HTTP providers
- SQLite WAL mode for concurrent reads

### Accessibility
- UI follows WAI-ARIA guidelines
- All interactive elements keyboard-accessible
- Colour-blind-friendly classification indicators

---

## Open Questions

1. ~~**Graph Engine:** SQLite with property-graph schema (recommended) vs. Kuzu embedded?~~ **DECIDED:** SQLite + FTS5 + `sqlite-vec` for vector search. Single database for graph, full-text, and vector queries.
2. ~~**JS Sandbox:** QuickJS (recommended — smaller) vs. Deno/V8 (richer API, larger binary)?~~ **DECIDED:** QuickJS. Tiny footprint (~200KB), simple cross-platform build, sufficient for sandboxed custom stages that only need `ctx.tools.call()`, `ctx.model.complete()`, and `ctx.state`.
3. ~~**Frontend Framework:** Solid (recommended for performance) vs. React (larger ecosystem) vs. Svelte?~~ **DECIDED:** SolidJS. Fine-grained reactivity (no virtual DOM), smallest bundle size, fastest rendering — ideal for a responsive desktop app.
4. ~~**Relay Hosting:** Self-hosted relay for peering, or integrate with existing service (Tailscale, Cloudflare Tunnel)?~~ **DECIDED:** Tailscale / Cloudflare Tunnel. Zero infrastructure for users, built-in NAT traversal and encrypted tunnels. No self-hosted relay server required.
5. ~~**Registry Hosting:** GitHub Releases-based (simpler) vs. dedicated registry server (more features)?~~ **DECIDED:** GitHub Releases-based. Each loop is a GitHub repo, discovered via GitHub Topics/search. Sigstore signing already integrates with GitHub OIDC. No custom infrastructure.
6. ~~**Messaging Auth:** How to securely map Discord/Slack users to HiveMind OS identity?~~ **DECIDED:** One-time verification code linking flow. User runs `/link` in Discord/Slack/Telegram, HiveMind OS DMs a 6-digit code, user confirms in the HiveMind OS UI. Simple, no OAuth infrastructure needed.
7. ~~**Linux Support:** The spec targets macOS + Windows. Should we add Linux as a first-class platform?~~ **DECIDED:** Yes — Linux is a first-class platform. Near-zero incremental effort with Tauri (WebKitGTK). Adds CI/CD targets and `.deb`/`.AppImage`/`.rpm` packaging.

---

## Risk Register

| Risk | Impact | Mitigation |
|---|---|---|
| **Tauri v2 maturity** | Platform bugs, missing APIs | Pin to stable release; maintain Electron fallback plan for critical gaps |
| **llama.cpp build complexity** | Cross-platform C++ compilation issues | Use pre-built binaries where possible; Candle as pure-Rust fallback |
| **Noise/QUIC transport** | Complex crypto implementation | Use well-audited crates (`snow`, `quinn`); extensive fuzz testing |
| **Sigstore ecosystem** | `sigstore-rs` crate maturity | Contribute upstream; fall back to shelling out to `cosign` CLI |
| **MCP spec evolution** | Breaking changes in MCP protocol | Abstract behind internal trait; version negotiation in client |
| **Knowledge graph scale** | SQLite performance with large graphs | Benchmark early; Kuzu as high-performance alternative backend |
| **Cross-platform daemon** | launchd vs Windows Service differences | Abstract behind `DaemonManager` trait; extensive platform testing |
| **QuickJS sandbox escapes** | Security risk for custom loop stages | Regular updates; capability-based restrictions; code review for SDK surface |

---

## Success Criteria (v1.0)

A user can:

1. ✅ Install HiveMind OS on macOS or Windows — daemon runs in background
2. ✅ Configure multiple model providers (local + cloud) with classification
3. ✅ Have a conversation with classification-aware model routing
4. ✅ Agent remembers facts in the knowledge graph and recalls them later
5. ✅ Connect MCP servers and use their tools in conversations
6. ✅ Schedule a background task that runs on a cron schedule
7. ✅ Install and run a community loop from the registry
8. ✅ Define a custom role and spin up agents in that role
9. ✅ Two agents communicate and delegate work
10. ✅ Pair with another HiveMind OS instance and sync knowledge
11. ✅ Receive agent notifications on Discord/Slack from their phone
12. ✅ Private data never leaks to public channels (verified by audit log)
