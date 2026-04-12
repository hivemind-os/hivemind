# hive-cli

Command-line interface for [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent. Provides daemon control and configuration management.

## Commands

### Daemon Management

```sh
# Start the daemon (optionally specify binary path or URL)
hive daemon start [--daemon-bin PATH] [--url URL]

# Check whether the daemon is running
hive daemon status

# Stop the daemon
hive daemon stop
```

### Configuration

```sh
# Display the current configuration
hive config show [--path PATH]

# Validate a configuration file
hive config validate [--path PATH]
```

## Dependencies

| Crate | Role |
|-------|------|
| `hive-core` (workspace) | Daemon control, config loading |
| `clap` | CLI argument parsing |
| `reqwest` | HTTP communication with the daemon |
| `serde` | Serialization / deserialization |
| `anyhow` | Error handling |

## Design Philosophy

`hive-cli` is an intentionally thin wrapper. All heavy lifting—process management, configuration resolution, AI orchestration—is delegated to `hive-core` and the daemon process. The CLI's only job is to parse arguments, call the appropriate `hive-core` function, and present the result.
