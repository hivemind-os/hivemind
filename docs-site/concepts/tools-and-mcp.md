# Tools & MCP

HiveMind OS comes loaded with built-in tools — and can connect to virtually any external service through the Model Context Protocol (MCP).

## Built-in Tools

Every HiveMind OS installation ships with a rich set of tools the agent can call to get things done:

| | Category (prefix) | What It Does |
|---|---|---|
| 📁 | **filesystem.\*** | Read, write, list, search, and glob files on your machine |
| 💻 | **shell.\*** | Execute commands in a sandboxed environment with configurable approval |
| ⚙️ | **core.\*** | Core agent operations and internal utilities |
| 🧠 | **knowledge.\*** | Query, create, and update nodes in the [knowledge graph](./knowledge-graph) |
| 🌐 | **http.\*** | Fetch URLs and make HTTP requests |
| 📝 | **json.\*** | JSON parsing, querying, and transforms |
| 🔢 | **math.\*** | Mathematical operations and calculations |
| 🕐 | **datetime.\*** | Date and time utilities |
| 🔄 | **process.\*** | Process management and execution |
| ⚡ | **workflow.\*** | Workflow orchestration and management |
| 💬 | **comm.\*** | Communication and notifications |
| 📅 | **calendar.\*** | Calendar access and scheduling |
| 👤 | **contacts.\*** | Contact management |
| 💾 | **drive.\*** | Drive and storage access |
| 🔌 | **connector.\*** | External service connectors |

These tools are always available — no setup required. The agent calls them automatically as part of its [agentic loop](./agentic-loops).

## What Is MCP?

::: info MCP in Plain English
**Model Context Protocol** is an open standard for connecting AI agents to external tools and data sources. Think of it as **USB ports for your AI** — plug in any compatible server and the agent instantly gains new capabilities, from browsing GitHub repos to querying databases.

MCP servers communicate over two transport types:

- **stdio** — runs as a local process on your machine (fast, private)
- **SSE / Streamable HTTP** — connects to a remote server over HTTP (great for shared corporate tools)
:::

When you connect an MCP server, HiveMind OS automatically discovers its tools, resources, and prompts. They become first-class actions the agent can use — just like built-in tools.

## Adding an MCP Server

1. Open **Settings → MCP Servers → Add**
2. Choose a transport type (stdio for local, HTTP for remote)
3. Fill in the connection details and save

Here's an example configuration that adds three MCP servers:

```yaml
mcp_servers:
  - id: filesystem
    transport: stdio
    command: npx @modelcontextprotocol/server-filesystem /Users/me/projects
    channel_class: local-only

  - id: github
    transport: stdio
    command: npx @modelcontextprotocol/server-github
    env:
      GITHUB_TOKEN: env:GITHUB_TOKEN
    channel_class: internal

  - id: corporate-kb
    transport: streamable-http
    url: https://internal.corp/mcp
    headers:
      Authorization: "Bearer ${CORP_TOKEN}"
    channel_class: internal
```

::: tip Popular MCP Servers to Try
- **GitHub** — manage repos, PRs, and issues directly from conversation
- **Filesystem** — enhanced file browsing with directory trees and search
- **Brave Search** — web search without leaving HiveMind OS
- **Postgres / SQLite** — query your databases in natural language
- **Slack** — read and send messages across your workspace

Browse the full directory at [MCP Servers Directory](https://github.com/modelcontextprotocol/servers).
:::

## Tool Approval Policies

Not every tool should run without oversight. HiveMind OS lets you set a **policy** for each tool:

| Policy | Behaviour |
|--------|-----------|
| **Auto** | Tool runs immediately without asking — best for trusted, read-only tools |
| **Ask** | Prompts you for confirmation before running (default for new tools) |
| **Deny** | Tool is blocked entirely — it won't appear in the agent's available actions |

You can configure these per-tool in your agentic loop config:

```yaml
tool_policy:
  auto_approve:
    - filesystem.read
    - github.get_issue
  require_confirmation:
    - filesystem.write
    - github.create_pr
  deny:
    - shell.execute
```

## Channel Classification on Tools

MCP servers get a **channel classification level** just like other outbound channels. The same data-classification rules apply: the agent will **never** send `CONFIDENTIAL` data to a server classified as `public`.

This means you can safely mix trusted internal servers with public ones — HiveMind OS enforces the boundaries automatically. A GitHub server classified as `internal` can see your private repo names, but a public search server won't receive anything beyond `PUBLIC` data.

## OS-Level Sandboxing

MCP servers that run as local processes (stdio transport) are executed inside an **OS-level sandbox** that restricts what the process can access on your machine:

| Platform | Mechanism |
|----------|-----------|
| **macOS** | `sandbox-exec` with generated Seatbelt profiles |
| **Linux** | Landlock (kernel 5.13+) with bubblewrap fallback |
| **Windows** | Restricted tokens (low integrity) + Job Objects |

The sandbox enforces:

- **Filesystem restrictions** — the process can only read and write to explicitly allowed paths. Sensitive directories like `~/.ssh`, `~/.aws`, `~/.gnupg`, and `~/.kube` are denied by default.
- **Network control** — network access can be allowed or blocked per server.
- **System path access** — standard system directories (like `/usr`, `/bin`, or `C:\Windows`) are allowed read-only so the process can function, but nothing beyond that.

Sandboxing is **enabled by default**. If a sandbox mechanism isn't available on the host, the process falls back to running unsandboxed.

::: tip Customising the sandbox
You can grant additional read or write paths per MCP server if it needs access to specific directories. See the [MCP Servers Guide](/guides/mcp-servers) for configuration details.
:::

## Example: Summarise Your Open PRs

Connect the GitHub MCP server and try this:

> *"Summarise my open pull requests and flag any that have been waiting for review for more than 3 days."*

HiveMind OS will call the GitHub MCP server's tools to list your PRs, check review status, and present a neat summary — all without you writing a single API call.

## Managed Runtimes

Many tools and MCP servers require **Node.js** or **Python** to run. HiveMind OS ships with [managed runtimes](/concepts/managed-runtimes) that are downloaded automatically — the agent uses these instead of relying on your system-installed versions. This ensures consistent behaviour for shell commands, process execution, and MCP stdio servers like `npx @modelcontextprotocol/server-github`.

## Learn More

- [Configure MCP Servers](/guides/mcp-servers) — Detailed setup guide for MCP
- [Privacy & Security](./privacy-and-security) — How classification protects your data across tools
- [Agentic Loops](./agentic-loops) — How the agent decides which tools to call
