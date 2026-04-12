# Managed Runtimes

HiveMind OS ships with **managed Node.js and Python environments** so agents can run code, install packages, and build projects out of the box — no manual setup required.

## Why Managed Runtimes?

When the agent needs to run a shell command like `npm install`, `node app.js`, or `python script.py`, it needs those runtimes available on `PATH`. Rather than relying on whatever happens to be installed on your system, HiveMind OS downloads and manages its own isolated copies of Node.js and Python.

This means:

- **Consistent behaviour** — every user gets the same runtime versions regardless of their system setup.
- **No conflicts** — the managed runtimes don't interfere with your system-installed Node.js or Python.
- **Zero configuration** — both runtimes are enabled by default and downloaded automatically on first launch.

## How It Works

On daemon startup, HiveMind OS checks for each enabled runtime:

1. **Already installed?** — If a matching version exists under `~/.hivemind/runtimes/`, it uses that.
2. **Not yet installed?** — It downloads the correct version for your platform and architecture automatically.
3. **PATH injection** — The managed runtime's `bin` directory is prepended to the agent's `PATH`, so all tools (`shell.execute`, `process.start`, MCP stdio servers) use the managed version by default.

Both runtimes coexist — Node.js and Python each get their own `PATH` entry, so commands like `node`, `npm`, `npx`, `python`, `pip`, and `uv` all resolve to the managed versions.

::: tip What about my system Node.js / Python?
The managed runtimes are **prepended** to `PATH`, so they take priority. Your system-installed versions still exist and are still accessible — they just appear later in `PATH`. If you disable a managed runtime, the agent falls back to whatever is on your system `PATH` (if anything).
:::

## What Uses the Managed Runtimes?

| Use Case | Example |
|---|---|
| **Shell commands** | Agent runs `npm install` or `python script.py` via the `shell.execute` tool |
| **Process tool** | Agent spawns `node server.js` via `process.start` |
| **MCP stdio servers** | An MCP server configured with `command: npx @modelcontextprotocol/server-github` uses the managed Node.js |
| **Package installation** | Agent installs npm or pip packages into a project it's building |
| **Python virtual environments** | Each session gets an isolated venv with common packages pre-installed |

### Building Projects

When the agent is building a Node.js or Python project (e.g. running `npm run build` or `pip install -r requirements.txt`), it uses the **managed runtime**. This ensures consistent builds regardless of what's installed on your host system.

## Configuration

Both runtimes are configured in `~/.hivemind/config.yaml`:

```yaml
# Python runtime
python:
  enabled: true
  python_version: "3.12"
  uv_version: "0.6.14"
  auto_detect_workspace_deps: true
  base_packages:
    - requests
    - beautifulsoup4
    - pandas
    - numpy
    - pyyaml
    - python-dateutil
    - Pillow
    - matplotlib
    - jinja2

# Node.js runtime
node:
  enabled: true
  node_version: "22.16.0"
```

### Python Options

| Field | Default | Description |
|---|---|---|
| `enabled` | `true` | Enable or disable the managed Python environment |
| `python_version` | `"3.12"` | Python version to install |
| `uv_version` | `"0.6.14"` | Version of the [uv](https://github.com/astral-sh/uv) package manager |
| `base_packages` | *(see above)* | Packages pre-installed in every session's virtual environment |
| `auto_detect_workspace_deps` | `true` | Automatically install dependencies from `requirements.txt` or `pyproject.toml` found in the workspace |

### Node.js Options

| Field | Default | Description |
|---|---|---|
| `enabled` | `true` | Enable or disable the managed Node.js environment |
| `node_version` | `"22.16.0"` | Node.js version to install (includes `npm` and `npx`) |

## Installation Paths

Managed runtimes are stored under your HiveMind home directory:

```
~/.hivemind/runtimes/
├── node/
│   └── node-v22.16.0-{platform}-{arch}/   # Node.js distribution
│       └── bin/                             # node, npm, npx
├── python/
│   ├── default/                             # Default Python venv
│   └── sessions/
│       └── <session-id>/                    # Per-session venv
└── uv                                       # uv package manager binary
```

## Disabling a Runtime

If you don't need one of the runtimes, disable it in your config:

```yaml
node:
  enabled: false
```

When disabled, the agent falls back to your system-installed version (if available). MCP servers that require that runtime will also fall back to the system version.

## Reinstalling

You can reinstall either runtime from the **Settings** UI or via the API:

- **Node.js** — `POST /api/v1/node/reinstall`
- **Python** — `POST /api/v1/python/reinstall`

This removes the existing installation and downloads a fresh copy. Useful if the runtime becomes corrupted or you change the configured version.

## See Also

- [Tools & MCP](/concepts/tools-and-mcp) — How the agent uses tools and connects to MCP servers
- [Configuration Reference](/reference/configuration) — Full config file reference
