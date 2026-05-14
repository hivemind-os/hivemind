# Copilot instructions for HiveMind OS

These instructions are for GitHub Copilot and other AI coding agents working in this repository. Prefer small, well-scoped changes; follow the existing crate/package boundaries; and verify behavior in the narrowest layer that covers your change.

## 1. Project overview

HiveMind OS is a privacy-first desktop AI agent and automation platform that runs primarily as a local daemon and exposes a Tauri + SolidJS desktop app, a CLI, local/workspace-aware tools, MCP integrations, workflows, personas, and a TypeScript-based plugin system. The desktop app is a thin client; the daemon owns the API, orchestration, model routing, tool execution, and most runtime state.

## 2. Repository layout

| Path | Purpose |
|---|---|
| `apps\hivemind-desktop\` | Tauri v2 + SolidJS desktop application. `src\` is the frontend, `src-tauri\` is the Rust bridge layer. |
| `crates\` | Rust workspace for the daemon, API, chat runtime, loop engine, tool system, models, workflows, plugins, MCP, and supporting services. |
| `packages\` | TypeScript packages for plugin development, registry metadata, test plugins, samples, and scaffolding. |
| `docs-site\` | VitePress documentation site for end users. Do not use this for contributor-only docs. |
| `tests\` | Workspace-level fixtures and integration support data. |
| `scripts\` | Build, packaging, publishing, and runtime staging scripts. |
| `vendor\` | Vendored crate patches (`rmcp`, `rmcp-macros`). Workspace `[patch.crates-io]` points here; do not modify casually. `cargo xtask fetch-models` may also stage model assets here. |
| `xtask\` | `cargo xtask` utilities for version checks, installer builds, daemon builds, and vendored model fetches. |
| `tools\` | Developer utilities such as `mock-mcp-server` (TypeScript) for MCP testing. |

### Key Rust crates

This is not exhaustive, but these are the crates most likely to matter when making changes:

| Crate | Purpose |
|---|---|
| `crates\hive-daemon\` | Thin production binary that loads config, initializes logging/audit state, builds the Axum router, and runs the local daemon. |
| `crates\hive-cli\` | CLI for daemon control and config inspection/validation. |
| `crates\hive-api\` | Axum HTTP API exposing `/api/v1` routes for chat, config, MCP, tools, models, workflows, scheduler, plugins, and more. |
| `crates\hive-chat\` | Chat/session orchestration, persona-aware tool wiring, session state, and loop integration. |
| `crates\hive-loop\` | Agentic loop engine with pluggable strategies and middleware; the currently exposed ReAct/Sequential/Plan-then-execute logic lives under `src\legacy\`. |
| `crates\hive-contracts\` | Shared serializable DTOs used across daemon, API, Tauri bridge, and frontend. |
| `crates\hive-core\` | Config loading, path discovery, audit logging, event bus, daemon control, and bundled personas/workflows. |
| `crates\hive-model\` | Model router, provider abstractions, routing decisions, and classification-aware model selection. |
| `crates\hive-inference\` | Local inference runtimes, Hugging Face Hub integration, hardware detection, and model registry primitives. |
| `crates\hive-local-models\` | Service layer for installed local models, downloads, hub search, and hardware/resource snapshots. |
| `crates\hive-knowledge\` | SQLite-backed property graph with FTS5 and vector search for long-term memory. |
| `crates\hive-tools\` | `Tool` trait, `ToolRegistry`, built-in tools, workflow tools, connector bridges, and MCP tool registration. |
| `crates\hive-mcp\` | MCP client, transport handling, persistent catalog, and per-session lazy connection management. |
| `crates\hive-plugins\` | Node.js plugin host for TypeScript connector plugins over JSON-RPC 2.0 on stdio. |
| `crates\hive-connectors\` | Connector abstractions, registries, provider adapters, services, secrets, and audit/state helpers. |
| `crates\hive-workflow\` | Workflow definition/validation, execution, shadow execution, persistence, and test running. |
| `crates\hive-workflow-service\` | Higher-level workflow service, triggers, event emission, and workflow/chat integration. |
| `crates\hive-scheduler\` | Scheduled task service with tool and agent execution hooks plus SQLite persistence. |
| `crates\hive-skills\` | Agent Skills discovery, parsing, indexing, and source adapters. |
| `crates\hive-skills-service\` | Persona-scoped skills discovery, installation, sync, and audit service. |
| `crates\hive-workspace-index\` | Workspace watchers and knowledge-graph indexing for files and embeddings. |
| `crates\hive-agent-kit\` | Agent kit export/import helpers and workflow reference rewriting. |
| `crates\hive-agents\` | Agent runner, supervisor, topology, naming, and telemetry primitives. |
| `crates\hive-canvas\` | Spatial canvas clustering, layout, events, and persistence support. |

### TypeScript packages

| Path | Purpose |
|---|---|
| `packages\plugin-sdk\` | TypeScript SDK for authoring HiveMind plugins, including tool definitions, Zod config schemas, loops, and testing helpers. |
| `packages\plugin-registry\` | Registry metadata (`registry.json`) used by the app’s plugin browser. |
| `packages\test-plugin\` | Host-API exercise plugin used for E2E and interoperability testing. |
| `packages\sample-plugins\` | Sample/reference plugins; currently includes a GitHub Issues connector example. |
| `packages\create-plugin\` | Scaffolding CLI for new HiveMind plugins. |

## 3. Build, test, and dev commands

Run these from the repo root unless noted otherwise.

### Rust workspace

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo check --package <crate>
cargo run -p hive-daemon
```

Notes:
- CI also runs `cargo xtask check-version`.
- `cargo check --package <crate>` is the fastest normal edit/compile loop for one crate.
- If you only changed one crate, run that crate’s tests first: `cargo test -p <crate>`.

### Desktop app

```powershell
cd apps\hivemind-desktop
npm install
npm run build
cargo tauri dev
```

Notes:
- CI uses `npm ci` in `apps\hivemind-desktop\`.
- `cargo tauri dev` is the full desktop hot-reload loop; `npm run build` is the frontend-only build.

### Plugin SDK and plugin packages

```powershell
cd packages\plugin-sdk
npm install
npm run build
```

Related package builds used in CI:

```powershell
cd packages\test-plugin
npm install
npm run build
```

## 4. Code conventions

### Rust formatting and style

- The workspace uses `rustfmt.toml` with `max_width = 100`, `newline_style = "Native"`, and `use_small_heuristics = "Max"`.
- Prefer existing crate/module boundaries over adding new top-level crates or cross-cutting utility modules.
- Names are consistently snake_case for modules, files, functions, and route handlers.

### Error handling

- Library crates commonly define typed errors with `thiserror::Error` (for example `hive-loop`, `hive-agents`, `hive-workflow`, `hive-tools`, `hive-mcp`).
- Setup, I/O, and binary/service orchestration code often uses `anyhow::{Result, Context}` to attach filesystem/network/runtime context (for example `hive-daemon` startup and `hive-core::bundled`).
- In API handlers, convert internal errors to `(StatusCode, String)` with the existing helper functions instead of inventing new response shapes.

### Async and runtime patterns

- Tokio is the standard async runtime across the workspace.
- Async traits are implemented with `async-trait` where trait methods need async behavior.
- Axum handlers are `async fn`; blocking database/filesystem work is often wrapped with `tokio::task::spawn_blocking`.
- Async tests commonly use `#[tokio::test]`.

### Naming and organization patterns

- API route modules live under `crates\hive-api\src\routes\`; request/query/response structs are usually named `*Request`, `*Query`, and `*Response`.
- Tool IDs are canonical dot-separated strings such as `filesystem.read`, `core.ask_user`, `plugin.<plugin_id>.<tool_name>`, and `mcp.<server_id>.<tool_name>`.
- Persona IDs are slash-delimited namespaces such as `system/general` and `system/software/planner`.
- Keep public cross-boundary types in `crates\hive-contracts\` instead of duplicating shapes in multiple crates.

### Frontend (SolidJS + TypeScript)

- The desktop frontend uses SolidJS function components and store factories with `createSignal`, `createMemo`, `createEffect`, `Show`, `For`, and lazy imports.
- TypeScript is in strict mode in the desktop app and plugin packages.
- The frontend uses the `~` alias for `apps\hivemind-desktop\src\`.
- Use `invoke()` for Tauri commands and `authFetch()` when the webview needs to talk to the daemon HTTP API through the Rust bridge.
- Keep shared frontend data shapes in `apps\hivemind-desktop\src\types.ts`.

### Testing conventions

- Prefer the narrowest test loop first: `cargo check -p <crate>`, then `cargo test -p <crate>`, then broader workspace/E2E coverage if needed.
- Rust integration tests live in crate-local `tests\` directories and use descriptive filenames like `knowledge_integration.rs`, `engine_integration.rs`, and `chat_agent_integration.rs`.
- Desktop E2E tests live under `apps\hivemind-desktop\tests\` and use Playwright `*.spec.ts` naming.
- Use `data-testid` attributes for UI that needs reliable E2E selection.

## 5. Key architectural patterns

- The daemon is the central service. The desktop app and CLI are clients; the desktop app is explicitly a thin client and talks to the daemon over HTTP rather than embedding business logic.
- `crates\hive-contracts\` is the shared contract surface for backend/frontend/Tauri boundaries.
- The agentic loop is in `crates\hive-loop\`. `LoopExecutor` selects a strategy (ReAct, Sequential, Plan-then-execute, or CodeAct) from the context, and middleware layers handle compaction, token budgets, classification, risk scanning, and stall detection.
- The runtime tool system is centered on `crates\hive-tools\::Tool` and `ToolRegistry`. Session-specific registries are assembled in `crates\hive-chat\src\chat.rs::build_session_tools()`.
- Built-in tools, connector tools, MCP tools, and plugin tools all end up in the same `ToolRegistry` surface.
- The plugin system is process-based: `crates\hive-plugins\` spawns Node.js child processes, communicates via JSON-RPC 2.0 over stdin/stdout, and bridges plugin tools back into the Rust tool registry.
- MCP uses both a global service and per-session managers. `McpCatalogStore` caches discovered tool/resource/prompt metadata; `SessionMcpManager` lazily connects servers on first use and disconnects them when a session ends.
- Bundled personas and workflows are embedded from `crates\hive-core\bundled-personas\` and `crates\hive-core\bundled-workflows\` via `include_str!`/`include_dir!` in `crates\hive-core\src\bundled.rs`.
- Workspace files are indexed into the knowledge graph through `crates\hive-workspace-index\`, not by ad hoc per-feature indexing.

## 6. Common tasks (recipes)

### How to add a new API route

1. Find the closest route module in `crates\hive-api\src\routes\` and add the handler there.
2. Follow existing Axum signatures: use `State<AppState>`, `Path<_>`, `Query<_>`, and `Json<_>` extractors instead of manual parsing.
3. Add typed request/query/response structs near the handler.
4. For blocking DB/filesystem work, use `tokio::task::spawn_blocking` like the knowledge-graph handlers do.
5. Wire the route in `crates\hive-api\src\lib.rs` inside `build_router()`.
6. If the new route crosses a frontend/Tauri boundary, add or reuse DTOs in `crates\hive-contracts\` and update `apps\hivemind-desktop\src\types.ts` or the Tauri wrapper layer.
7. Add focused tests in the relevant crate test module or integration tests.

### How to add a new built-in tool

1. Implement the tool in `crates\hive-tools\src\<name>_tool.rs` and implement the `Tool` trait (`definition()` + `execute()`).
2. Export the module/type from `crates\hive-tools\src\lib.rs`.
3. Register it where tools are assembled:
   - static/default registry setup in `crates\hive-chat\src\chat.rs::with_model_router()`
   - per-session runtime registry in `crates\hive-chat\src\chat.rs::build_session_tools()`
4. If the tool should appear in tool discovery or settings UIs, make sure it is visible through the existing `/api/v1/tools` surface.
5. Add unit tests for the tool and, if relevant, session tool wiring tests.
6. Keep tool metadata accurate: approval mode, channel class, side effects, and annotations matter.

### How to add a new persona

1. Add the persona YAML under `crates\hive-core\bundled-personas\`.
2. Register it in `crates\hive-core\src\bundled.rs` by adding an `include_str!` entry to `BUNDLED_PERSONA_YAMLS`.
3. If the persona ships bundled skills, create `bundled-personas\<namespace>\<name>\skills\<skill-name>\SKILL.md` (plus any assets/scripts) and add the corresponding `include_dir!` + `bundled_skill_dir()` match arm.
4. Keep the persona ID, loop strategy, allowed tools, preferred models, and MCP server assignments aligned with the existing schema.
5. Validate by loading personas through the daemon/config flow or by exercising persona-related API/UI paths.

### How to modify the chat / reasoning loop

1. Start in `crates\hive-loop\src\legacy\strategies\` for the current strategy implementations (`react.rs`, `sequential.rs`, `plan_then_execute.rs`, `code_act.rs`).
2. Strategy selection is in `crates\hive-loop\src\legacy\strategy.rs`; loop context and errors are in `legacy\types.rs` and the crate-level middleware files.
3. Prefer changing strategy-local logic or middleware over sprinkling behavior into callers.
4. If context/tool wiring changes, update the integration points in `crates\hive-chat\` as well.
5. Add or update focused tests in `crates\hive-loop\tests\` and `crates\hive-loop\src\legacy\tests.rs`.
6. Treat behavior changes here as high-risk: this code affects tool execution, agent behavior, and user-visible reasoning.

### How to add a new desktop UI feature

1. Put UI in `apps\hivemind-desktop\src\components\` and reusable stateful logic in `apps\hivemind-desktop\src\stores\` or `src\lib\`.
2. Follow the existing SolidJS style: typed props/interfaces, `createSignal`/`createMemo`/`createEffect`, and `Show`/`For` instead of React-style state patterns.
3. Update shared frontend types in `apps\hivemind-desktop\src\types.ts`.
4. If you need backend bridge changes, add a `#[tauri::command(rename_all = "snake_case")]` handler in `apps\hivemind-desktop\src-tauri\src\lib.rs`.
5. Use `invoke()` for Tauri commands and `authFetch()` for daemon HTTP access from the webview.
6. Add `data-testid` for important UI controls and update Playwright/Vitest coverage where appropriate.

## 7. Do’s and Don’ts

### Do

- Do use `cargo check --package <crate>` for fast feedback while iterating on one crate.
- Do read `README.md`, `TESTING_GUIDE.md`, and the relevant crate/package READMEs before making architectural changes.
- Do run `cargo test -p <crate>` for the crate you changed before widening to workspace-level tests.
- Do keep shared wire-format types in `crates\hive-contracts\`.
- Do preserve the daemon-first architecture: prefer extending the daemon/API/contracts rather than pushing logic into the desktop shell.

### Don’t

- Don’t modify files under `vendor\` unless the task is explicitly about updating vendored code or assets.
- Don’t put contributor/developer guidance in `docs-site\`; that site is the end-user documentation source.
- Don’t make broad, casual changes to the reasoning loop, tool execution path, or agent behavior without focused tests and a clear need.
- Don’t add new dependencies without justification; there are already dedicated crates for most concerns in this workspace.
- Don’t duplicate contract shapes between Rust and TypeScript when `crates\hive-contracts\` or `src\types.ts` should be the source of truth.

## 8. Testing

For full guidance, read `TESTING_GUIDE.md`. Note: some paths and crate names in the testing guide may be stale; prefer the commands in this file and CI workflow for current accuracy.

### Practical testing summary

- Start with the smallest loop that covers your change:
  - `cargo check -p <crate>` for compile/type feedback
  - `cargo test -p <crate>` for crate-local logic
  - `cargo test --workspace` for cross-crate integration confidence
- Use targeted test selection when possible:

```powershell
cargo test -p hive-classification -- restricted_defaults_to_block
cargo test -p hive-api --test knowledge_integration
```

- For frontend work, use the desktop app’s existing scripts in `apps\hivemind-desktop\package.json`:

```powershell
cd apps\hivemind-desktop
npm run test:unit
npm run test:e2e:integration
npm run test:e2e:cdp
```

- Rust unit tests should cover classification logic, routing, workflow state, parsing, storage, and other deterministic logic.
- Integration tests should cover subsystem boundaries (daemon/API, MCP, workflows, knowledge graph, model routing, plugin host behavior, etc.).
- Real-model credentials are not required for normal PR checks; the testing guide expects the default workspace test loop to work with mock or scripted providers.
- Desktop E2E tests use Playwright `*.spec.ts` files under `apps\hivemind-desktop\tests\`; keep names descriptive and feature-scoped.
- If a UI change affects selector stability, add or preserve `data-testid` hooks.
