# hive-mcp

Model Context Protocol (MCP) client for [HiveMind OS](../../README.md) — a cross-platform, privacy-aware desktop AI agent.

## What is MCP?

The [Model Context Protocol](https://modelcontextprotocol.io/) is an open standard that lets AI agents interact with external tools, resources, and prompts through a unified interface. `hive-mcp` implements the client side of this protocol, allowing HiveMind OS to discover and invoke capabilities exposed by any MCP-compatible server.

## API Overview

**`McpService`** is the main entry point for all MCP operations:

| Method | Returns | Description |
|---|---|---|
| `list_servers()` | `Vec<McpServerSnapshot>` | List all configured MCP servers and their connection status |
| `connect_server(id)` | `McpServerSnapshot` | Connect to a server by ID |
| `disconnect_server(id)` | `McpServerSnapshot` | Disconnect from a server |
| `list_tools(id)` | `Vec<McpToolInfo>` | List tools exposed by a connected server |
| `call_tool(id, name, args)` | `McpCallToolResult` | Invoke a tool on a connected server |
| `list_resources(id)` | `Vec<McpResourceInfo>` | List resources available on a server |
| `list_prompts(id)` | `Vec<McpPromptInfo>` | List prompt templates from a server |
| `list_notifications(limit)` | `Vec<McpNotificationEvent>` | Retrieve recent server notifications |

Errors are represented by `McpServiceError`:

- **`ServerNotFound`** — no server exists with the given ID
- **`NotConnected`** — the server is configured but not currently connected
- **`ConnectionFailed`** — connection attempt failed
- **`RequestFailed`** — a request to a connected server failed

## Transports

`hive-mcp` supports three MCP transport types:

- **Stdio** — communicates with a local subprocess over stdin/stdout. Best for locally-installed tools and scripts.
- **SSE (Server-Sent Events)** — communicates with a remote server over HTTP. Suited for shared or cloud-hosted MCP servers.
- **Streamable HTTP** — communicates with a remote server using the MCP Streamable HTTP protocol. Combines HTTP POST for requests with SSE for server-initiated messages.

Each server maintains its own connection state (`McpServerState`) with status tracking. A notification queue (capped at 200 events) collects async messages from servers, integrated through the event bus.

## Dependencies

### Workspace crates

| Crate | Purpose |
|---|---|
| `hive-core` | Core types and utilities |
| `hive-contracts` | Shared trait definitions |
| `hive-classification` | Tool/model classification support |

### External

| Crate | Purpose |
|---|---|
| `rmcp` | MCP protocol implementation |
| `tokio` | Async runtime |
| `thiserror` | Error derive macros |
| `serde` / `serde_json` | Serialization |
| `tracing` | Structured logging |
