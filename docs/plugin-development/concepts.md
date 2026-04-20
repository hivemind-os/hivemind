# Plugin Architecture & Concepts

## How Plugins Work

A Hivemind plugin runs as a **separate Node.js process** that communicates with the host via **JSON-RPC 2.0 over stdio**. This provides:

- **Process isolation** — a plugin crash doesn't affect the host or other plugins
- **Language flexibility** — the protocol is language-agnostic (TypeScript SDK provided)
- **MCP compatibility** — plugin tools use the standard MCP `tools/list` and `tools/call` methods

```
┌─────────────────────┐     stdio JSON-RPC    ┌──────────────────────┐
│   Hivemind Host     │ ◄──────────────────► │   Plugin Process     │
│   (Rust/Tauri)      │                       │   (Node.js)          │
│                     │  Host → Plugin:       │                      │
│  • PluginHost       │  • initialize         │  • definePlugin()    │
│  • PluginRegistry   │  • tools/list         │  • Tool handlers     │
│  • PluginBridgeTool │  • tools/call         │  • Background loop   │
│  • MessageRouter    │  • plugin/startLoop   │  • Lifecycle hooks   │
│                     │  • plugin/activate    │                      │
│                     │                       │                      │
│                     │  Plugin → Host:       │                      │
│                     │  • host/emitMessage   │                      │
│                     │  • host/secretGet     │                      │
│                     │  • host/emitEvent     │                      │
│                     │  • host/updateStatus  │                      │
│                     │  • host/log           │                      │
└─────────────────────┘                       └──────────────────────┘
```

## Plugin Lifecycle

```
Install → Initialize → Activate → Running → Deactivate → Uninstall
                                     ↑
                          startLoop ←┘ (optional)
```

1. **Install** — Plugin npm package is installed into the Hivemind plugins directory
2. **Initialize** — Host spawns the plugin process, sends config and host info
3. **Activate** — `onActivate` hook runs (validate credentials, warm caches)
4. **Running** — Tools are registered, loop can be started
5. **Deactivate** — `onDeactivate` hook runs (cleanup)
6. **Uninstall** — Plugin files are removed

## Key Components

### definePlugin()

The main entry point. When your `index.ts` is loaded, `definePlugin()`:
1. Sets up the JSON-RPC transport on stdin/stdout
2. Registers handlers for all host→plugin methods
3. Begins listening for commands

### Config Schema

A Zod schema that describes your plugin's settings. The host serializes this schema to JSON and renders it as a form in the Settings UI. Fields marked `.secret()` are stored in the OS keyring, not in config files.

### Tools

Functions the AI agent can call. Each tool has:
- **name** — unique within the plugin
- **description** — shown to the AI agent
- **parameters** — Zod schema for input validation
- **annotations** — side-effect and approval metadata
- **execute** — the actual implementation

Tools are registered with the host as `plugin.<pluginId>.<toolName>`.

### Background Loop

An optional long-running function that can:
- Poll external APIs for updates
- Emit incoming messages into the Hivemind connector pipeline
- Emit events that trigger workflow automations
- Persist sync cursors for restart resilience

The loop receives an `AbortSignal` and should use `ctx.sleep()` for cancellation-aware delays.

### Host APIs

The `PluginContext` (`ctx`) provides access to host capabilities:

| Category | APIs |
|----------|------|
| Messaging | `emitMessage`, `emitMessages` |
| Secrets | `secrets.get`, `secrets.set`, `secrets.delete`, `secrets.has` |
| Storage | `store.get`, `store.set`, `store.delete`, `store.keys` |
| Logging | `logger.debug`, `logger.info`, `logger.warn`, `logger.error` |
| Notifications | `notify` |
| Events | `emitEvent` |
| Status | `updateStatus` |
| HTTP | `http.fetch` |
| File System | `dataDir.readFile`, `dataDir.writeFile`, etc. |
| Discovery | `connectors.list`, `personas.list` |
| Host Info | `host.version`, `host.platform`, `host.capabilities` |

## Message Flow

When a plugin emits a message via `ctx.emitMessage()`:

```
Plugin emitMessage() → host/emitMessage RPC
    → PluginMessageRouter
    → Deduplication (by source field)
    → Classification (personal/work/automated)
    → Persona routing (based on connector config)
    → Workflow triggers (custom events)
    → Desktop notification
    → Agent inbox
```

## Plugin Isolation

Each plugin gets:
- Its own Node.js process
- Scoped secret storage (can't read other plugins' secrets)
- Private data directory (`~/.hivemind/plugins/<id>/data/`)
- Declared permissions (displayed to user at install time)

## Relation to MCP

The plugin protocol is a **superset of MCP**:
- `tools/list` and `tools/call` are standard MCP methods
- Plugin extensions (`plugin/*`, `host/*`) add connector-specific capabilities
- A plugin is also a valid MCP server for tool-only use cases
