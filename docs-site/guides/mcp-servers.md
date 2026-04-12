# MCP Servers

Connect HiveMind OS to external tools and data sources through the **Model Context Protocol (MCP)**. This guide walks you through adding servers, configuring transports, and scoping access.

## Adding an MCP Server via UI

1. Open **Settings → MCP Servers**
2. Click **Add**
3. Choose a transport type — **stdio** (local process), **SSE** (remote HTTP), or **streamable-http** (bidirectional HTTP)
4. Fill in the connection details (command, args, URL, environment variables)
5. Assign a **channel classification** (see [below](#channel-classification))
6. Click **Save** — HiveMind OS connects and discovers the server's tools automatically

## Adding via YAML Config

You can also define MCP servers directly in your configuration file.

### stdio Transport (Local Process)

Use `stdio` for servers that run as a local process on your machine. HiveMind OS spawns the process and communicates over stdin/stdout.

```yaml
mcpServers:
  - id: filesystem
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/home/user/projects"]
    channel_class: local-only
```

### SSE Transport (Remote HTTP)

Use `sse` for remote servers accessible over HTTP. Ideal for shared corporate tools or cloud-hosted services.

```yaml
mcpServers:
  - id: remote-api
    transport: sse
    url: https://my-mcp-server.example.com/sse
    channel_class: internal
```

### Streamable HTTP Transport

Use `streamable-http` for servers that support bidirectional HTTP streaming — a newer alternative to SSE with better connection handling.

```yaml
mcpServers:
  - id: streaming-api
    transport: streamable-http
    url: https://my-mcp-server.example.com/mcp
    channel_class: internal
```

### Environment Variables

Pass secrets via `env` — reference OS-level environment variables with the `env:` prefix so keys stay out of config files:

```yaml
mcpServers:
  - id: github
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-github"]
    env:
      GITHUB_TOKEN: env:GITHUB_TOKEN
    channel_class: internal
```

## Popular MCP Servers

::: tip Great Servers to Try
Browse the full directory at [MCP Servers Directory](https://github.com/modelcontextprotocol/servers).
:::

| Server | What It Does | Install |
|---|---|---|
| **filesystem** | Enhanced file browsing and search | `npx @modelcontextprotocol/server-filesystem` |
| **github** | GitHub repos, PRs, and issues | `npx @modelcontextprotocol/server-github` |
| **postgres** | Query PostgreSQL databases | `npx @modelcontextprotocol/server-postgres` |
| **brave-search** | Web search via Brave | `npx @modelcontextprotocol/server-brave-search` |
| **slack** | Read and send Slack messages | `npx @modelcontextprotocol/server-slack` |

## Channel Classification

::: warning Classify Every MCP Server
Every MCP server must be assigned a `channel_class`. This controls what data the agent is allowed to send to it. A misconfigured classification can leak sensitive information to a public endpoint.
:::

HiveMind OS applies the same [data-classification rules](/concepts/privacy-and-security) to MCP servers as to model providers:

| Channel Class | Accepts Data Up To |
|---|---|
| `public` | `PUBLIC` only |
| `internal` | `PUBLIC`, `INTERNAL` |
| `private` | `PUBLIC`, `INTERNAL`, `CONFIDENTIAL` |
| `local-only` | All levels (never leaves your machine) |

**Example:** A filesystem server accessing local files should be `local-only`. A remote API you don't fully control should be `internal` or `public` depending on trust level.

## Per-Persona MCP Scoping

You can assign specific MCP servers to individual [personas](/concepts/personas). This limits which integrations each persona can access — a security-focused reviewer doesn't need your Slack server.

```yaml
id: user/data-analyst
name: Data Analyst
mcpServers:
  - id: postgres
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-postgres"]
    channel_class: internal
  - id: filesystem
    transport: stdio
    command: npx
    args: ["-y", "@modelcontextprotocol/server-filesystem", "/data"]
    channel_class: local-only
allowed_tools:
  - mcp.postgres.*
  - filesystem.read
```

Only the servers listed in `mcpServers` are available to that persona. Each entry is a full MCP server configuration object. Omit the field (or use `*` in `allowedTools`) to grant access to all connected servers.

## Troubleshooting

| Problem | What to Check |
|---|---|
| **Server not starting** | Verify the `command` path is correct and the binary is installed (e.g., `npx` is on your PATH) |
| **Tools not appearing** | Restart the MCP connection from **Settings → MCP Servers** — tool discovery runs at connect time |
| **Permission errors** | Check the server's `channel_class` — it may be too restrictive for the data the agent needs to send |
| **Timeout issues** | Increase the connection timeout in server settings; remote SSE servers may need longer initial handshakes |

## Learn More

- [Tools & MCP](/concepts/tools-and-mcp) — How built-in tools and MCP work together
- [Privacy & Security](/concepts/privacy-and-security) — Data classification and channel enforcement
- [Personas Guide](/guides/personas) — Scoping tools and servers per persona
