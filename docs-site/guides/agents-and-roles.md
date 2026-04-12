# Agents & Roles

HiveMind OS lets you create specialised agent personas — each with their own tools, knowledge access, and personality — then spin up live instances that work autonomously and talk to each other.

## Agent Roles

An **AgentRole** defines the high-level function of an agent. HiveMind OS provides these built-in roles:

| Role | Purpose |
|---|---|
| `planner` | High-level planning and task decomposition |
| `researcher` | Deep web/knowledge research, read-heavy |
| `coder` | Code writing and modification |
| `reviewer` | Code review, read-only analysis |
| `writer` | Content creation, documentation, prose |
| `analyst` | Data analysis and reporting |
| `custom(string)` | Any custom role you define by name |

Roles are assigned on `AgentSpec` or `BotConfig` when spawning an agent — not on personas. See [Personas Guide](/guides/personas) for persona configuration and [Bots Guide](/guides/bots) for bot setup.

## Spawning and Managing Agents

Launch agents from the UI or programmatically using the built-in agent management tools.

**From the desktop app:**
- Open a new chat session and select a persona with the desired role
- Use the Bots page to launch persistent autonomous agents

**Programmatically via tools:**

Agents are spawned and managed using these core tools:

| Tool | Purpose |
|---|---|
| `core.spawn_agent` | Launch a new agent instance with a given persona/role |
| `core.wait_for_agent` | Block until a spawned agent completes |
| `core.signal_agent` | Send a signal or message to a running agent |
| `core.get_agent_result` | Retrieve the result of a completed agent |
| `core.list_agents` | List all running agent instances |
| `core.kill_agent` | Terminate a running agent |
| `core.list_personas` | List available personas |

## Multi-Agent Patterns

### Pipeline

Agents hand off work sequentially — each stage feeds into the next:

```
User Request → [Researcher] → findings → [Coder] → code → [Reviewer] → approved PR
```

An orchestrating agent can implement this pattern by using `core.spawn_agent` to launch each stage and `core.wait_for_agent` to block until each completes before feeding results to the next.

### Fan-Out / Fan-In

Spawn multiple agents in parallel, then merge results:

1. Use `core.spawn_agent` multiple times to launch parallel workers
2. Use `core.wait_for_agent` on each to collect results
3. Use `core.get_agent_result` to retrieve and merge outputs

### Supervision

A parent agent spawns child agents and monitors them:

1. Spawn children with `core.spawn_agent`
2. Monitor with `core.list_agents`
3. Send guidance with `core.signal_agent`
4. Terminate if needed with `core.kill_agent`

::: tip
Mix patterns freely. A pipeline stage can fan out into parallel workers, and merged results can feed into the next pipeline stage.
:::

## The Agent Dashboard

The **Agents** view in the UI gives you a live table of every running agent instance — its role, status, and current task:

```
┌──────────┬──────────────┬──────────┬────────────────┐
│ Instance │ Role         │ Status   │ Task           │
├──────────┼──────────────┼──────────┼────────────────┤
│ agent-01 │ 🔍 Reviewer  │ Working  │ Review PR #42  │
│ agent-02 │ 💻 Coder     │ Working  │ Implement cache│
│ agent-03 │ 📚 Researcher│ Idle     │ —              │
│ agent-04 │ 🏗️ Planner   │ Working  │ Supervising    │
└──────────┴──────────────┴──────────┴────────────────┘
```

Click any agent to view its conversation history and workflow state.

::: tip
All inter-agent communication respects data classification. An agent cannot send data that exceeds the recipient's clearance level.
:::

## What's Next?

- **[Agentic Loops](./agentic-loops.md)** — Customise how agents reason and plan
- **[Knowledge Management](./knowledge-management.md)** — Configure memory and knowledge retrieval
- **[Security Policies](./security-policies.md)** — Data classification rules for agents
- **[Workflows](./workflows.md)** — Build automation pipelines
