::: tip 🖥️ Looking for the UI walkthrough?
This page covers the developer/CLI experience. If you're using the desktop app, see [First Five Minutes](/getting-started/first-five-minutes).
:::

# First Five Minutes (Developer)

You've installed HiveMind OS, added a provider, and the daemon is humming. Now let's see what this thing can do! Here are five things to try right now — each takes under a minute.

---

## 🔍 1. Ask It to Research Something

HiveMind OS isn't a search engine — it's a research assistant. Give it a meaty question and watch it synthesize a real answer.

**Try this:**

```
Research the pros and cons of using Rust vs Go for a new CLI tool. Give me a summary table.
```

**What to expect:** A structured comparison table covering performance, ecosystem, learning curve, compile times, and more — with a short analysis after it. The agent may use web tools to pull in current data if they're connected.

---

## 🧠 2. Teach It Something

HiveMind OS has a knowledge graph that grows over time. You can teach it your preferences by telling it in natural language.

**Try this:**

```
I prefer TypeScript with strict mode, React for UI, and PostgreSQL for databases. Please remember that.
```

**What to expect:** HiveMind OS notes the preference. From now on, whenever you ask it to scaffold a project, pick a stack, or review code, it'll default to your preferred technologies.

::: tip Slash Commands
Use `/prompt` (or `/p`) to load a saved prompt template into the conversation.
:::

---

## 📄 3. Read and Analyze a File

Point HiveMind OS at any file and ask it to think critically. It uses built-in filesystem tools to read the file directly — no copy-paste needed.

**Try this:**

```
Read the README.md in this directory and suggest 3 improvements
```

**What to expect:** The agent reads the file using its filesystem tool, then gives you three concrete, actionable suggestions — anything from missing sections to unclear wording to better badge placement.

---

## 🔌 4. Connect an MCP Server

MCP (Model Context Protocol) servers give HiveMind OS new superpowers. Let's add the filesystem server for enhanced directory browsing and file search.

**Try this:**

MCP servers are configured per-persona. Open your HiveMind OS config (`~/.hivemind/config.yaml`) and add an MCP server to a persona:

```yaml
personas:
  - id: dev-assistant
    name: Dev Assistant
    mcp_servers:
      - id: filesystem
        transport: stdio
        command: npx @modelcontextprotocol/server-filesystem /path/to/your/projects
        channel_class: local-only
```

Replace `/path/to/your/projects` with your actual projects directory. HiveMind OS discovers the server's tools automatically on next restart — no extra wiring needed.

**What to expect:** New tools like `filesystem.list`, `filesystem.read`, and `filesystem.search` appear in your Tools panel. The agent can now browse and search your project files natively.

::: tip Bonus
There are MCP servers for GitHub, browsers, databases, and more. See the [MCP Servers guide](/guides/mcp-servers) for the full catalog.
:::

---

## 🎭 5. Create Your First Persona

Personas are reusable agent identities with their own system prompt, model preferences, and tool access. Think of them as specialized coworkers.

**Try this:**

```
Create a persona called 'Code Reviewer' that focuses on security and code quality
```

**What to expect:** HiveMind OS generates a persona definition with a security-focused system prompt, conservative model temperature, and scoped tool access (read-only filesystem, linting, and diff tools — no write access). You can switch to this persona any time you want a thorough code review.

---

## You're Just Getting Started

These five experiments barely scratch the surface. HiveMind OS can schedule background tasks, run autonomous bots, and build multi-step workflows.

Ready to go deeper? Explore the **Concepts** section:

- [How It Works](/concepts/how-it-works) — Architecture and the agentic loop
- [Knowledge Graph](/concepts/knowledge-graph) — How memory and recall work
- [Personas](/concepts/personas) — Build specialized agent identities
- [Tools & MCP](/concepts/tools-and-mcp) — Extend HiveMind OS with any tool
- [Workflows](/concepts/workflows) — Automate multi-step processes
