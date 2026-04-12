# hive-api

HTTP API server for [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent. This crate exposes daemon functionality as REST endpoints built on [Axum](https://github.com/tokio-rs/axum).

## API Routes

All routes are prefixed with `/api/v1` unless noted otherwise.

| Group | Routes | Description |
|---|---|---|
| **Health** | `GET /healthz` | Liveness probe |
| **Daemon** | `GET /daemon/status`, `POST /daemon/shutdown` | Version, uptime, shutdown |
| **Config** | `GET /config/get`, `PUT /config`, `GET /config/validate` | Read, update and validate HiveMind OS configuration |
| **Chat** | `GET\|POST /chat/sessions`, `GET /chat/sessions/{id}` | Session CRUD |
| | `POST /chat/sessions/{id}/messages` | Send a message (starts agentic loop) |
| | `POST /chat/sessions/{id}/interrupt` | Interrupt a running session |
| | `POST /chat/sessions/{id}/resume` | Resume an interrupted session |
| | `GET /chat/sessions/{id}/memory` | Retrieve session memory |
| | `GET /chat/sessions/{id}/risk-scans` | Prompt-injection risk scan records |
| **Model Router** | `GET /model/router` | Current model router snapshot |
| **MCP** | `GET /mcp/servers` | List configured MCP servers |
| | `POST /mcp/servers/{id}/connect\|disconnect` | Connect / disconnect a server |
| | `GET /mcp/servers/{id}/tools\|resources\|prompts` | Server capabilities |
| | `GET /mcp/notifications` | MCP notification log |
| **Tools** | `GET /tools`, `POST /tools/{id}/invoke` | List and invoke available tools |
| **Local Models** | `GET /local-models`, `GET /local-models/search` | List installed and search hub models |
| | `POST /local-models/install`, `DELETE /local-models/{id}` | Install / remove a local model |
| | `GET /local-models/hardware` | Hardware capability summary |
| **Scheduler** | `GET\|POST /scheduler/tasks`, `GET\|DELETE /scheduler/tasks/{id}` | Scheduled task CRUD |
| | `POST /scheduler/tasks/{id}/cancel` | Cancel a running task |
| **Knowledge** | `GET\|POST /knowledge/nodes`, `GET\|DELETE /knowledge/nodes/{id}` | Knowledge graph node CRUD |
| | `GET /knowledge/nodes/{id}/edges` | Edges for a node |
| | `POST /knowledge/edges`, `DELETE /knowledge/edges/{id}` | Edge CRUD |
| | `GET /knowledge/search`, `GET /knowledge/stats` | Search and statistics |
| **Memory** | `GET /memory/search` | Semantic memory search |

## Key Types

| Type | Description |
|---|---|
| `AppState` | Central application state shared across all handlers. Holds `Arc` references to every service and an `Arc<Notify>` for graceful shutdown. |
| `ChatService` | Manages chat sessions, message dispatch, and agentic loop execution. |
| `ChatRuntimeConfig` | Runtime-tunable chat parameters. |
| `SendMessageRequest` / `SendMessageResponse` | Request and response types for the chat message endpoint. |
| `RiskService` | Prompt-injection scanning via `RiskScanRecord`. |
| `LocalModelService` | Download, install, and manage local inference models. |
| `SchedulerService` | Persistent task scheduling backed by SQLite. |
| `build_router()` | Constructs the full Axum `Router` with all routes and a permissive CORS layer. |

## Feature Flags

| Flag | Effect |
|---|---|
| `candle` | Enables the Candle inference runtime (forwards to `hive-inference/candle`) |
| `llama-cpp` | Enables the llama.cpp inference runtime (forwards to `hive-inference/llama-cpp`) |
| `onnx` | Enables the ONNX inference runtime (forwards to `hive-inference/onnx`) |

None are enabled by default.

## Dependencies

### Workspace (internal)

`hive-core` · `hive-classification` · `hive-contracts` · `hive-inference` · `hive-loop` · `hive-mcp` · `hive-model` · `hive-tools` · `hive-knowledge`

### External

`axum` · `tokio` · `tower-http` (CORS) · `rusqlite` · `sha2` · `serde` / `serde_json` · `parking_lot` · `reqwest` · `tracing` · `thiserror` · `anyhow`

## Architecture

```
┌─────────────┐
│  Axum Router │  ← build_router()
└──────┬──────┘
       │  with_state(AppState)
       ▼
┌─────────────────────────────────────────┐
│              AppState                    │
│  ┌────────┐ ┌─────┐ ┌─────────────┐    │
│  │  Chat  │ │ MCP │ │ LocalModels │ …  │
│  └────────┘ └─────┘ └─────────────┘    │
│        Arc<Notify> → graceful shutdown  │
└─────────────────────────────────────────┘
```

All services are initialised at startup inside `AppState::new()` and shared via `Arc`. The daemon shuts down gracefully when the `shutdown` `Notify` is triggered.
