# CLI Overview

HiveMind OS includes a command-line interface (`hive`) for managing the daemon, validating configuration, and checking for updates.

## Installation

The `hive` CLI ships bundled with the HiveMind OS desktop application. After installing the desktop app, the `hive` command is available in your terminal.

Verify with:

```bash
hive --help
```

## Basic Usage

```bash
hive <command> [subcommand] [options]
```

Run any command with `--help` for detailed usage:

```bash
hive daemon --help
```

## Available Commands

| Command | Description |
|---|---|
| `hive daemon` | Start, stop, and manage the background daemon |
| `hive config` | Show or validate your configuration |
| `hive update` | Check for available updates |

## How It Works

The `hive` CLI manages the HiveMind OS daemon — the background process that powers everything. The daemon starts automatically with the desktop app, or you can launch it independently:

```bash
hive daemon start
```

Most day-to-day interaction happens in the **desktop app** or via the daemon's **HTTP API** (`http://localhost:9180` by default). The CLI is primarily for daemon lifecycle management and scripting.

## What's Next

- [CLI Commands Reference](./commands.md) — full list of commands and options
