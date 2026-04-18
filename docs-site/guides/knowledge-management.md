# Knowledge Management

HiveMind OS builds a persistent knowledge graph from your conversations. You can teach it explicitly, let it learn automatically, and query it at any time.

## Teaching the Agent

Tell the agent what you want it to remember using natural language in the chat:

```text
Remember that I prefer TypeScript with strict mode
Our API uses REST with JSON, auth via JWT tokens
The production database is PostgreSQL 15 on AWS RDS
```

The agent uses its internal memory system to store facts as nodes in the knowledge graph. You don't need any special commands — just ask naturally and the agent will persist important context.

::: tip
Be specific. "Remember that our deploy target is ECS Fargate in us-east-1" is more useful than "We use AWS".
:::

## Automatic Knowledge Extraction

During context compaction HiveMind OS automatically extracts entities, relationships, and facts from your conversations — no action required. Over time the graph grows to include technologies, architecture decisions, team conventions, and more.

## Searching and Browsing Knowledge

### Natural Language Queries

Ask the agent to recall information in natural language:

```text
What database do we use in production?
What technologies have we discussed in our architecture sessions?
```

The agent uses the `knowledge.query` tool internally to search the knowledge graph. This tool supports two actions:

- **search** — full-text search across knowledge nodes
- **explore** — retrieve a specific node and its neighbors in the graph

### Knowledge Explorer UI

You can also browse the full knowledge graph visually in the **Knowledge Explorer** — a dedicated UI view that lets you navigate nodes, relationships, and clusters interactively.

## Knowledge Scopes and Namespaces

Knowledge is organized into scopes so the right facts surface in the right context:

| Scope | Applies to | Example |
|---|---|---|
| **Global** | All workspaces and sessions | General preferences like UI theme |
| **Workspace** | A specific project directory | Project-specific tools and conventions |

Workspace-scoped knowledge activates automatically when you open a session inside that project. Facts remembered while working in a project are automatically scoped to that workspace.

::: info
Shared namespaces let teams pool knowledge across workspaces. See [Personas](./personas) for namespace configuration.
:::

## Classification and Data Protection

Every knowledge node inherits the **data classification** of the conversation it came from:

| Classification | Behavior |
|---|---|
| `PUBLIC` | Available to all providers |
| `INTERNAL` | Available to internal providers |
| `CONFIDENTIAL` | Prompted before sharing externally |
| `RESTRICTED` | Never sent to external providers |

This means a fact learned in a `RESTRICTED` session will never leak to a `PUBLIC` provider — classification travels with the knowledge automatically.

::: warning
If a workspace is classified as `RESTRICTED`, all facts remembered inside it inherit that classification. Review workspace classification in **Settings → Workspace → Classification**.
:::

## Agent Kits

To share agent configurations across environments, use **Agent Kits**. Kits export personas, workflows, skills, and attachments as a portable `.agentkit` ZIP file — but not knowledge graph data. Knowledge is managed separately per instance.

See the [Agent Kits guide](/guides/agent-kits) for full details on exporting, importing, and namespace remapping.

## Next Steps

- [Personas](./personas) — create specialized agents that leverage stored knowledge
- [MCP Servers](./mcp-servers) — connect external tools the agent can remember how to use
- [Privacy & Security](../concepts/privacy-and-security) — learn more about data classification
