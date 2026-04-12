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
npm run tauri dev
```

### Building for Production

```bash
npm run tauri build
```

The compiled application bundle will be placed in `src-tauri/target/release/bundle/`.
