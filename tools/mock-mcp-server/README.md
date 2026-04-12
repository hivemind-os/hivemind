# Mock MCP Server

A configurable mock MCP server for manual testing of HiveMind OS's MCP client integration. It exposes mock tools with canned responses and a web dashboard for real-time monitoring and control.

## Quick Start

```bash
cd tools/mock-mcp-server
npm install
npm run compile

# Stdio mode (default) — use with HiveMind OS's stdio MCP transport
npm start

# HTTP mode — use with HiveMind OS's SSE or Streamable HTTP transport
npm start -- --mode http --port 6100
```

## HiveMind OS Configuration

Add to your HiveMind OS MCP server config to test against the mock server:

### Stdio transport
```yaml
mcp_servers:
  - id: test-server
    transport: stdio
    command: node
    args: ["tools/mock-mcp-server/dist/index.js", "--dashboard-port", "6100"]
```

### SSE transport
Start the server first: `npm start -- --mode http --port 6100`
```yaml
mcp_servers:
  - id: test-server
    transport: sse
    url: http://localhost:6100/sse
```

### Streamable HTTP transport
Start the server first: `npm start -- --mode http --port 6100`
```yaml
mcp_servers:
  - id: test-server
    transport: streamable-http
    url: http://localhost:6100/mcp
```

> **Note:** The `url` field must include the scheme (`http://`). Omitting it will cause a URL parse error.

## Transport Modes

### Stdio (default)
Reads/writes MCP JSON-RPC messages via stdin/stdout. The monitoring dashboard is served on a separate HTTP port (default: 6100).

### HTTP
MCP protocol served over HTTP with both SSE and Streamable HTTP transports. The dashboard is served on the same port.

| Endpoint | Method | Transport |
|----------|--------|-----------|
| `/mcp` | GET/POST/DELETE | Streamable HTTP (2025-03-26) |
| `/sse` | GET | SSE stream (2024-11-05) |
| `/messages` | POST | SSE message endpoint |

## Dashboard

Open `http://localhost:6100/dashboard` in a browser to access the monitoring UI.

**Features:**
- **Request Log** — Live-updating table of all MCP tool calls with timestamps, arguments, status, duration, and response content
- **Tool Response Overrides** — Per-tool dropdown to select which canned response to return
- **Global Controls** — Response delay slider, failure rate slider, pause/resume toggle
- **Connected Clients** — Shows active MCP client connections and their transport type

## Mock Tools

| Tool | Description | Responses |
|------|-------------|-----------|
| `get_weather` | Weather lookup by city | sunny, rainy, city not found, timeout |
| `search_database` | Query database records | results, empty, query error |
| `send_email` | Send email message | sent, bounced, rate limited |
| `calculate` | Evaluate math expression | result, division by zero, overflow |
| `file_operations` | Read/write/list files | content, listing, write success, not found, permission denied |

Each tool has a default response and multiple canned alternatives selectable from the dashboard.

## Testing

```bash
# Node.js protocol test
cd tools/mock-mcp-server && node test-stdio.mjs

# Rust integration test (tests stdio, SSE, and Streamable HTTP against rmcp)
cargo test -p hive-mcp --test mock_mcp_server
```

## CLI Options

```
mock-mcp-server [options]

  -m, --mode <stdio|http>       Transport mode (default: stdio)
  -p, --port <number>           HTTP port (default: 6100)
  -d, --dashboard-port <number> Dashboard port in stdio mode (default: 6100)
      --delay <ms>              Default response delay (default: 0)
      --fail-rate <0-1>         Random failure rate (default: 0)
  -h, --help                    Show help
```
