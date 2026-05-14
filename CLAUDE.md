# CLAUDE.md

This file provides guidance for Claude Code and other AI coding agents working on the HiveMind OS codebase.

## Project overview

HiveMind OS is a privacy-first, local-first AI agent platform centered on a daemon process. The runtime exposes a local HTTP API, a Tauri + SolidJS desktop app, a CLI, workspace-aware tools, MCP integrations, workflows, personas, and a TypeScript plugin system.

The desktop app is a thin client. The daemon owns API composition, orchestration, model routing, tool execution, workflow/runtime state, and most business logic. Prefer small, well-scoped changes that respect existing crate and package boundaries.

## Build & test commands

Run from the repo root unless noted otherwise.

### Rust workspace

```powershell
cargo build --workspace
cargo test --workspace
cargo clippy --workspace
cargo check --package <crate>
cargo test -p <crate>
cargo run -p hive-daemon
cargo xtask check-version
```

### Desktop app

```powershell
cd apps\hivemind-desktop
npm install
npm run build
npm run test:unit
npm run test:e2e:integration
npm run test:e2e:cdp
cargo tauri dev
```

### Plugin packages

```powershell
cd packages\plugin-sdk
npm install
npm run build

cd ..\test-plugin
npm install
npm run build
```

### Fast iteration rule

Use the narrowest loop first:
1. `cargo check --package <crate>`
2. `cargo test -p <crate>`
3. broader workspace or E2E coverage only if needed

## Repository layout

| Path | Purpose |
| --- | --- |
| `apps\hivemind-desktop\` | Tauri v2 + SolidJS desktop app (`src\` frontend, `src-tauri\` Rust bridge). |
| `crates\` | Rust workspace: daemon, API, chat, loop, tools, models, workflows, plugins, MCP, services. |
| `packages\` | TypeScript packages for plugin SDK, registry, test/sample plugins, scaffolding. |
| `tests\` | Workspace-level fixtures and integration support data. |
| `scripts\` | Build, packaging, publishing, and staging scripts. |
| `xtask\` | `cargo xtask` utilities. |
| `docs-site\` | End-user docs source, not contributor guidance. |
| `vendor\` | Vendored crate patches (`rmcp`, `rmcp-macros`); do not modify. |
| `tools\` | Developer utilities (e.g. `mock-mcp-server` for MCP testing). |

## Architecture

For full details, read `ARCHITECTURE.md`. Keep these facts in mind:

- `hive-daemon` boots the system and runs the local service.
- `hive-api` is the composition root: it builds `AppState`, wires services, and constructs the Axum router.
- `hive-chat` is the main interactive runtime: sessions, persona resolution, session tool assembly, event flow, and orchestration.
- `hive-loop` contains the agentic chat loop. Chat currently uses `hive-loop::legacy`; workflows use `hive-workflow::WorkflowEngine` (separate crate).
- `hive-contracts` is the shared contract surface for daemon, API, Tauri bridge, and frontend. Put shared DTOs here.
- `hive-model` routes LLM calls across providers and handles model selection decisions.
- `hive-tools` owns the `Tool` trait and `ToolRegistry`; built-in, MCP, connector, and plugin tools converge here.
- `hive-mcp` manages MCP transport, cataloging, and per-session lazy connections.
- `hive-plugins` hosts Node.js TypeScript plugins over JSON-RPC stdio and bridges them into the Rust tool system.
- `hive-workflow` + `hive-workflow-service` provide persisted workflow definitions, execution, triggers, and chat integration.
- `hive-workspace-index` indexes workspace files into the knowledge graph; avoid ad hoc indexing paths.
- The runtime tool set is assembled dynamically per session and persona, not from one static global list.
- Preserve the daemon-first architecture: desktop and CLI are clients, not places for core business logic.

## Code conventions

### Rust and architecture
- Follow existing crate/module boundaries; avoid new top-level crates unless clearly necessary.
- Use snake_case for modules, files, functions, and route handlers.
- Keep public cross-boundary types in `crates\hive-contracts\`.
- Use existing API helper patterns instead of inventing new response shapes.

### Error handling
- Prefer typed errors with `thiserror::Error` in library crates.
- Use `anyhow::{Result, Context}` for setup, I/O, and orchestration code.
- In Axum handlers, convert internal failures to `(StatusCode, String)` using existing helpers.

### Async patterns
- Tokio is the standard runtime.
- Use `async-trait` where traits need async methods.
- Wrap blocking DB/filesystem work with `tokio::task::spawn_blocking`.
- Use `#[tokio::test]` for async tests.

### Frontend and TypeScript
- Use SolidJS function components and the existing signal/store patterns.
- TypeScript runs in strict mode.
- Use the `~` alias for `apps\hivemind-desktop\src\`.
- Use `invoke()` for Tauri commands and `authFetch()` for daemon HTTP access.
- Keep shared frontend shapes in `apps\hivemind-desktop\src\types.ts`.

### Naming and testing
- API route modules live under `crates\hive-api\src\routes\`.
- Request/query/response structs usually use `*Request`, `*Query`, `*Response`.
- Tool IDs are dot-separated (`filesystem.read`, `mcp.<server_id>.<tool_name>`, `plugin.<plugin_id>.<tool_name>`).
- Persona IDs are slash-delimited (`system/general`, `system/software/planner`).
- Use descriptive Rust integration test names and Playwright `*.spec.ts` files.
- Add or preserve `data-testid` for UI that needs reliable E2E selectors.

## Common tasks

### Add an API route
1. Add the handler in the closest module under `crates\hive-api\src\routes\`.
2. Use normal Axum extractors (`State`, `Path`, `Query`, `Json`).
3. Add typed request/query/response structs nearby.
4. Wire the route in `crates\hive-api\src\lib.rs` inside `build_router()`.
5. If it crosses frontend/Tauri boundaries, update `crates\hive-contracts\` and frontend/Tauri types.
6. Add focused tests.

### Add a built-in tool
1. Implement the tool in `crates\hive-tools\src\` and implement `Tool`.
2. Export it from `crates\hive-tools\src\lib.rs`.
3. Register it in `crates\hive-chat\src\chat.rs` for default and/or session assembly.
4. Ensure discovery/settings surfaces still work if applicable.
5. Add tool and wiring tests.

### Add a persona
1. Add persona YAML under `crates\hive-core\bundled-personas\`.
2. Register it in `crates\hive-core\src\bundled.rs`.
3. Add bundled skills/assets if needed.
4. Keep persona IDs, loop strategy, tools, models, and MCP config aligned with schema.

### Modify chat / reasoning loop
1. Start in `crates\hive-loop\src\legacy\` for current chat strategies.
2. Prefer strategy-local or middleware changes over caller hacks.
3. Update `crates\hive-chat\` if tool/context wiring changes.
4. Add focused tests in `crates\hive-loop\tests\` and related legacy tests.

### Add a desktop UI feature
1. Put UI in `apps\hivemind-desktop\src\components\`.
2. Put reusable stateful logic in `src\stores\` or `src\lib\`.
3. Update shared frontend types.
4. Add Tauri bridge commands only when needed.
5. Add `data-testid` and relevant unit/E2E coverage.

## Important guidelines

### Do
- Use `cargo check --package <crate>` for fast feedback.
- Read `ARCHITECTURE.md` before making architectural changes.
- Read `TESTING_GUIDE.md` for broader test strategy (note: some paths in it are stale; prefer commands listed above).
- Run crate-local tests before workspace-wide tests.
- Keep daemon/API/contracts as the main extension point.
- Verify behavior in the narrowest layer that covers your change.

### Don't
- Do not modify `vendor\` unless the task is explicitly about vendored code.
- Do not put contributor guidance in `docs-site\`.
- Do not duplicate contract shapes across Rust and TypeScript.
- Do not make consequential changes to the reasoning loop, tool execution path, or agent behavior casually; treat them as high-risk and add focused tests.
- Do not add dependencies without clear justification; the workspace already has dedicated crates for many concerns.
