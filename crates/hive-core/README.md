# hive-core

Shared infrastructure layer for the HiveMind OS project — a cross-platform, privacy-aware desktop AI agent. This crate provides configuration loading, audit logging, event-based communication, and daemon lifecycle management used by the rest of the workspace.

## Overview

`hive-core` consolidates the foundational services that other HiveMind OS crates depend on. It owns no UI or AI logic; instead it supplies the plumbing every higher-level component needs: reading configuration files, writing tamper-evident audit logs, dispatching events between subsystems, and controlling the background daemon process.

## Modules

### `config`

Loads and validates YAML configuration from a platform-standard user-level directory and project-level (`.hivemind/hivemind.yaml`) files.

The user-level config location is determined in order of priority:
1. `HIVEMIND_CONFIG_PATH` env var (exact file path)
2. `HIVEMIND_HOME` env var (directory containing `config.yaml`)
3. `~/.hivemind/` (default home directory)

**Key functions:** `load_config()`, `load_config_with_cwd()`, `load_config_from_paths()`, `validate_config()`, `validate_config_file()`, `discover_paths()`, `ensure_paths()`, `hive_paths_from()`, `save_config()`, `config_to_yaml()`.

### `audit`

Tamper-evident audit trail for recording significant actions and decisions made by the agent.

**Key types:** `AuditLogger`, `AuditEntry`, `NewAuditEntry`.

### `event_bus`

Pub-sub event system for decoupled, inter-component communication within the HiveMind OS runtime.

**Key types:** `EventBus`, `EventEnvelope`, `TopicSubscription`.

### `daemon_control`

Lifecycle management for the HiveMind OS background daemon — starting, stopping, querying status, and resolving its URL.

**Key functions:** `daemon_start()`, `daemon_status()`, `daemon_stop()`, `daemon_url()`, `resolve_daemon_binary()`.

### `models` (re-exports)

Re-exports contract types from `hive-contracts` so downstream crates can depend on `hive-core` alone:

`HiveMindConfig`, `DaemonConfig`, `ModelProviderConfig`, `McpServerConfig`, `SecurityConfig`, `HiveMindPaths`, `DaemonStatus`, and many more.

## Dependencies

### Internal (workspace)

| Crate | Purpose |
|---|---|
| `hive-contracts` | Shared type definitions (configs, status, capabilities) |
| `hive-classification` | Classification utilities |

### External

| Crate | Purpose |
|---|---|
| `dirs` | Resolving platform-specific user directories |
| `reqwest` | HTTP client for daemon communication |
| `serde` / `serde_yaml` / `serde_json` | Serialization and config parsing |
| `sha2` | Hashing for audit integrity |
| `tokio` | Async runtime |
| `tracing` | Structured logging |

## Usage

`hive-core` is a workspace-internal crate. Add it as a path dependency in another HiveMind OS crate:

```toml
[dependencies]
hive-core = { path = "../hive-core" }
```

Then import what you need:

```rust
use hive_core::{load_config, validate_config, HiveMindConfig, AuditLogger, EventBus};
```
