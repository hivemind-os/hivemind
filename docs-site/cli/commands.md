# CLI Commands

Complete reference for the `hive` CLI.

## Daemon Management

Control the HiveMind OS background daemon.

### `hive daemon start`

Start the daemon process. If it's already running, prints a message and exits.

```bash
hive daemon start
hive daemon start --daemon-bin /path/to/hive-daemon    # Custom binary path
hive daemon start --url http://localhost:9180           # Custom bind URL
```

### `hive daemon status`

Check whether the daemon is running and display its info.

```bash
hive daemon status
```

Output includes PID, version, platform, bind address, and uptime.

### `hive daemon stop`

Shut down the running daemon.

```bash
hive daemon stop
hive daemon stop --no-restart    # Also unload the auto-start service
```

::: tip
Use `--no-restart` to prevent the system service from restarting the daemon after it stops. This is useful when you want the daemon to stay stopped until you explicitly start it again.
:::

### `hive daemon load`

Register the daemon to auto-start at login (uses launchd on macOS, Windows Service on Windows).

```bash
hive daemon load
```

### `hive daemon unload`

Unregister the daemon auto-start. The daemon won't restart after being stopped.

```bash
hive daemon unload
```

## Configuration

View and validate your HiveMind OS configuration.

### `hive config show`

Print the current resolved configuration as YAML.

```bash
hive config show
```

### `hive config validate`

Check whether your configuration is valid.

```bash
hive config validate                          # Validate the default config
hive config validate --path ./my-config.yaml  # Validate a specific file
```

## Updates

### `hive update`

Check for available updates and display download instructions.

```bash
hive update
```

Compares your installed version against the latest release and prints a download URL if an update is available.

## See Also

- [CLI Overview](./overview.md) — installation and usage
- [Configuration Reference](../reference/configuration.md) — full config schema
