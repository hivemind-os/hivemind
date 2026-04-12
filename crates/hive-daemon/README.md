# hive-daemon

Production daemon binary for **HiveMind OS** — a cross-platform, privacy-aware desktop AI agent.

`hive-daemon` is the main entry point that starts the HTTP server and bootstraps the entire system. It is intentionally a thin binary: all domain logic lives in library crates, and this crate only orchestrates startup, shutdown, and process lifecycle.

## Startup Flow

1. **Load configuration** — `hive_core::load_config()` reads and validates the HiveMind OS config.
2. **Discover & create paths** — config dir, audit log dir, knowledge graph dir, and other runtime directories.
3. **Initialize audit logging** — creates an `AuditLogger` and records the daemon start event.
4. **Build application state** — constructs `EventBus`, shutdown signal, and `AppState` (including any blocking HTTP clients, created *before* entering the async runtime to avoid nested runtimes).
5. **Write PID file** — persists the process ID for external status checks.
6. **Enter Tokio runtime** — starts background services (scheduler, model registry) and builds the Axum router via `hive_api::build_router()`.
7. **Bind & serve** — listens on the configured TCP address (`config.api.bind`) with graceful shutdown on Ctrl+C.
8. **Shutdown** — logs a shutdown audit entry, removes the PID file, and drops the runtime cleanly.

## Dependencies

### Workspace (internal) crates

| Crate | Role |
|---|---|
| `hive-api` | HTTP router, application state, and all inference backends (candle, llama-cpp, onnx) |
| `hive-core` | Configuration, paths, audit logging, event bus |
| `hive-classification` | Data classification types used in audit entries |

### External crates

| Crate | Role |
|---|---|
| `axum` | HTTP framework — router and server |
| `tokio` | Async runtime |
| `tracing` / `tracing-subscriber` | Structured logging with configurable log levels |
| `anyhow` | Ergonomic error handling |

## Design Philosophy

`hive-daemon` follows a **thin-binary** pattern:

- The binary contains no domain logic. It wires together library crates and manages the process lifecycle.
- All HTTP routes, business logic, inference, and persistence live in `hive-api`, `hive-core`, and sibling crates.
- This keeps the binary easy to audit, fast to compile incrementally, and simple to replace (e.g., with a test harness or alternative entry point).
