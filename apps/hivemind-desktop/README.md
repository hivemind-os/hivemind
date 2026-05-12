# hivemind-desktop

Cross-platform desktop UI for [HiveMind OS](../../README.md), a privacy-aware desktop AI agent. Built with **Tauri v2** and **SolidJS**.

The desktop app is a thin client — all intelligence lives in the HiveMind OS daemon. Communication happens over HTTP (not IPC), which keeps the architecture portable and enables future CLI/web clients.

## Technology Stack

| Layer    | Technology                              |
| -------- | --------------------------------------- |
| Frontend | SolidJS + TypeScript + Vite             |
| Backend  | Tauri v2 (webview wrapper) + Rust       |
| Build    | npm/pnpm + cargo                        |

## Project Structure

```
hivemind-desktop/
├── package.json          # Frontend deps (solid-js, @tauri-apps/api)
├── vite.config.ts
├── tsconfig.json
├── src/
│   ├── index.tsx         # Entry point
│   ├── App.tsx           # Root component
│   └── styles.css
└── src-tauri/
    ├── Cargo.toml        # Tauri + hivemind bridging crates
    └── src/
        ├── main.rs       # Tauri window bootstrapping
        └── lib.rs        # Command handlers (~40 invokable commands)
```

## Tauri Commands

All commands are defined in `src-tauri/src/lib.rs` and are async (via `tauri::async_runtime::spawn_blocking`). The frontend invokes them through `@tauri-apps/api/core::invoke()`.

### Daemon Control

| Command        | Description                  |
| -------------- | ---------------------------- |
| `daemon_status`| Check if the daemon is alive |
| `daemon_start` | Start the daemon process     |
| `daemon_stop`  | Stop the daemon process      |
| `config_show`  | Show current configuration   |
| `app_context`  | Get application context      |

### Chat

| Command                  | Description                     |
| ------------------------ | ------------------------------- |
| `chat_list_sessions`     | List all chat sessions          |
| `chat_create_session`    | Create a new chat session       |
| `chat_get_session`       | Get a specific session          |
| `chat_send_message`      | Send a message in a session     |
| `chat_interrupt`         | Interrupt an ongoing response   |
| `chat_resume`            | Resume a paused session         |
| `chat_get_session_memory`| Get memory for a session        |
| `chat_list_risk_scans`   | List risk scans for a session   |
| `memory_search`          | Search across session memory    |

### Model Management

| Command                | Description              |
| ---------------------- | ------------------------ |
| `model_router_snapshot`| Get model router state   |

### MCP (Model Context Protocol)

| Command                | Description                    |
| ---------------------- | ------------------------------ |
| `mcp_list_servers`     | List available MCP servers     |
| `mcp_connect_server`   | Connect to an MCP server       |
| `mcp_disconnect_server`| Disconnect from an MCP server  |
| `mcp_list_tools`       | List tools from MCP servers    |
| `mcp_list_resources`   | List resources from MCP servers|
| `mcp_list_prompts`     | List prompts from MCP servers  |
| `mcp_list_notifications`| List MCP notifications        |

### Tools

| Command      | Description           |
| ------------ | --------------------- |
| `tools_list` | List available tools  |

### Local Models

| Command                      | Description                        |
| ---------------------------- | ---------------------------------- |
| `local_models_list`          | List installed local models        |
| `local_models_get`           | Get details of a local model       |
| `local_models_install`       | Install a local model              |
| `local_models_remove`        | Remove an installed model          |
| `local_models_search`        | Search available models            |
| `local_models_hardware`      | Get hardware capabilities          |
| `local_models_resource_usage`| Get current resource usage         |
| `local_models_storage`       | Get model storage information      |

## Architecture

- **Thin client** — the desktop app contains no business logic; the HiveMind OS daemon owns all AI, tool, and model operations.
- **HTTP transport** — Tauri commands call the daemon over HTTP via `reqwest::blocking::Client`, not Tauri IPC. This decouples the UI from the runtime and allows the same daemon API to serve CLI or web frontends.
- **Async commands** — every Tauri command spawns blocking work on the async runtime, keeping the UI thread responsive.
- **Graceful degradation** — on launch the app checks daemon status and starts it automatically if needed.
- **Reactive frontend** — SolidJS reactive primitives drive the UI; state updates flow through signals and effects rather than a virtual DOM.

## Development

### Prerequisites

- [Node.js](https://nodejs.org/) (LTS)
- [Rust](https://rustup.rs/) toolchain
- Tauri v2 prerequisites ([platform-specific guide](https://v2.tauri.app/start/prerequisites/))

### Getting Started

```bash
# Install frontend dependencies
npm install

# Run in development mode (starts both Vite dev server and Tauri window)
npm run tauri:dev
```

### Building for Production

```bash
npm run tauri:build
```

The compiled application bundle will be placed in `src-tauri/target/release/bundle/`.

## Frontend Patterns

These notes document patterns that are already in use in the current desktop app. Prefer matching them when adding features or refactoring existing code.

### 1. State management patterns

- `App.tsx` owns most app-wide state with `createSignal()` for daemon status, selected session, screen/tab selection, modal visibility, error banners, and busy state.
- Domain state in `src/stores/` is usually exposed as a **factory function** that returns Solid accessors, setters, memos, and actions:
  - `createWorkflowStore()`
  - `createBotStore()`
  - `createInteractionStore()`
  - `createAgentKitStore()`
  - `createConfigStore(...)`
  - `createWorkspaceStore(...)`
- Stores mainly use `createSignal`, `createMemo`, and `createEffect`. No `createStore()` usage was found in `src/`.
- Components usually consume state through **typed props** (`Accessor`, `Setter`, or a typed store object), not global context.
- The main exception is `themeStore.ts`, which exports a module-scope singleton signal for theme state.
- `src/ui/sidebar.tsx` uses `createContext()` internally for the sidebar primitive, but app feature state is not organized around context providers.

Example from `App.tsx`:

```ts
const [activeScreen, setActiveScreen] = createSignal<'session' | 'bots' | 'scheduler' | 'workflows' | 'settings' | 'agent-kits'>('session');
const [activeTab, setActiveTab] = createSignal<'chat' | 'workspace' | 'stage' | 'workflows' | 'events' | 'processes' | 'config' | 'mcp'>('chat');
const workflowStore = createWorkflowStore();
const botStore = createBotStore();
const interactionStore = createInteractionStore();
```

### 2. Navigation model

- Navigation is **signal-driven**, not URL/router-driven.
- `App.tsx` uses a top-level `activeScreen()` signal plus `<Switch>` / `<Match>` to choose the current screen.
- Main screens currently are:
  - `session`
  - `bots`
  - `scheduler`
  - `workflows`
  - `settings`
  - `agent-kits`
- Inside the session/bot detail view there is a second navigation layer via `activeTab()` for:
  - `chat`
  - `workspace`
  - `stage`
  - `workflows`
  - `events`
  - `processes`
  - `config` (bot-only)
  - `mcp`
- The sidebar drives navigation by receiving `activeScreen` and `setActiveScreen` as props. Selecting a session explicitly switches back to the `session` screen.

Example:

```tsx
<Switch>
  <Match when={activeScreen() === 'scheduler'}>
    <SchedulerPage ... />
  </Match>
  <Match when={activeScreen() === 'workflows'}>
    <WorkflowsPage ... />
  </Match>
  <Match when={activeScreen() === 'settings'}>
    <SettingsModal ... />
  </Match>
</Switch>
```

### 3. Backend communication

The frontend talks to the daemon in three main ways:

#### Tauri commands via `invoke()`

- Command-style operations use `invoke()` from `@tauri-apps/api/core`.
- Commands are defined in `src-tauri/src/lib.rs` with `#[tauri::command]` and include areas like chat, config, workflows, tools, MCP, workspace, and local models.
- Typical examples:
  - `config_get`
  - `chat_send_message`
  - `workspace_list_files`
  - `tools_list`
  - `workflow_subscribe_events`

#### HTTP daemon calls via `authFetch()`

- REST-style daemon endpoints should go through `src/lib/authFetch.ts`.
- `authFetch()` does **not** call the daemon directly from the browser. It proxies through the Tauri `daemon_fetch` command so production builds avoid mixed-content problems and reuse daemon auth handling.
- Feature code often adds a small domain wrapper on top of `authFetch()` (for example `kgFetch()` in knowledge-graph code).

Example:

```ts
const resp = await authFetch(`${url}${path}`, init);
if (!resp.ok) throw new Error(await resp.text());
return resp.json();
```

#### Real-time updates via daemon SSE -> Tauri events

- The daemon exposes SSE streams, but the frontend usually does **not** create a browser `EventSource` directly.
- Instead, Rust subscribes to SSE and re-emits Tauri events such as:
  - `workflow:event`
  - `interaction:event`
  - `stage:event`
- Frontend stores/components then subscribe with `listen()` from `@tauri-apps/api/event`.
- `src/lib/useSSESubscription.ts` exists as a lifecycle-safe helper, but many existing stores currently call `listen()` directly.

Practical rule:

- Use `invoke()` for explicit command handlers.
- Use `authFetch()` for daemon HTTP endpoints.
- Use Tauri `listen()` for live event streams.

### 4. Component organization

- `src/components/` contains feature and page components.
  - Top-level screens include things like `ChatView`, `WorkspaceView`, `BotsPage`, `SchedulerPage`, `WorkflowsPage`, `AgentKitsPage`, and `SettingsModal`.
  - There is clear feature grouping in subfolders such as:
    - `components/settings/`
    - `components/setup/`
    - `components/connectors/`
    - `components/workflow/`
    - `components/flight-deck/`
    - `components/shared/`
    - `components/plugins/`
- `src/ui/` contains reusable UI primitives such as `Button`, `Dialog`, `Tabs`, `Popover`, `Sidebar`, `Table`, and `Tooltip`.
- `src/lib/` contains non-visual helpers and integration code such as:
  - `authFetch.ts`
  - `useSSESubscription.ts`
  - syntax-highlighting helpers
  - routing/grouping utilities
  - MCP helpers
  - small focused unit tests for utility modules

In practice:

- Put feature behavior in `components/` or `stores/`.
- Put reusable presentational primitives in `ui/`.
- Put transport/helpers/hooks/utilities in `lib/`.

### 5. UI framework

- The app is built with **SolidJS** and leans heavily on Solid primitives:
  - `createSignal`
  - `createEffect`
  - `createMemo`
  - `Show`
  - `For`
  - `Switch` / `Match`
- `createResource()` is used occasionally for component-local async loading, but signals + explicit async functions are the dominant pattern.
- The component primitive layer is built on **Kobalte** (`@kobalte/core`) for accessible primitives (e.g. Dialog, DropdownMenu, Select) and wrapped locally in `src/ui/`. Not all components use Kobalte — many are plain SolidJS components.
- Styling is a mix of:
  - Tailwind-style utility classes directly in `class="..."`
  - shared UI variants with `class-variance-authority`
  - `clsx` + `tailwind-merge` via `cn()` in `src/lib/utils.ts`
  - global CSS variables and app-level styles in `src/styles.css`
- No CSS modules or styled-components files were found in this app.

### 6. Important conventions

#### Dialog dismissal

- Many dialogs intentionally prevent accidental dismissal with:

```tsx
<DialogContent onInteractOutside={(e) => e.preventDefault()} />
```

- Some long-running flows also block Escape until the action completes.
- Follow this pattern for destructive, multi-step, or in-progress dialogs.

#### Error handling

- Most async actions use `try/catch` and either:
  - set a local error signal (`error`, `hubSearchError`, `configLoadError`, etc.), or
  - log non-fatal issues with `console.warn` / `console.error`.
- `App.tsx` centralizes many user-facing action failures through `runAction()`, which sets `busyAction` and `errorMessage`.
- `ErrorBanner.tsx` special-cases HuggingFace token/license errors instead of treating them as generic failures.

#### Loading states

- Loading is usually explicit and local to the feature:
  - `loading`
  - `hubSearchLoading`
  - `installInProgress`
  - `workspaceLoading`
  - `fileSaving`
  - `auditRunning`
- The common pattern is `setLoading(true)` before async work and reset it in `finally`.

#### TypeScript strictness

- `tsconfig.json` has `"strict": true`.
- The codebase also leans on explicit union types for screen/tab state and typed `Accessor` / `Setter` props.

### Contributor guidance

When adding frontend code, prefer to:

1. Start with local `createSignal()` state.
2. Extract a store factory in `src/stores/` when state/actions are shared across a feature.
3. Pass accessors, setters, and store objects down through typed props.
4. Use `invoke()` for Tauri commands and `authFetch()` for daemon HTTP endpoints.
5. Route new top-level views through `activeScreen()` in `App.tsx` instead of adding a router.
6. Match the existing dialog, loading, and error-handling conventions.
