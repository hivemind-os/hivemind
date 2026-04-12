# Scheduling

HiveMind OS runs as a background daemon, which means it can execute tasks on your behalf even when the UI is closed. The scheduler supports several trigger types so you can automate anything from daily reminders to event-driven workflows.

## Task Schedule Types

Tasks use the `TaskSchedule` enum with three variants:

| Type | Description | Example |
|---|---|---|
| **Once** | Run immediately, one time only | Fire-and-forget tasks |
| **Scheduled** | Run once at a specific timestamp | `run_at_ms: 1733076000000` — run at a specific epoch time |
| **Cron** | Recurring cron expression | `expression: "0 9 * * MON-FRI"` — every weekday at 9 AM |

## Workflow Trigger Types

Workflows support a broader set of triggers:

| Trigger | Description |
|---|---|
| `manual` | Started by user action |
| `incoming_message` | Reacts to messages from connectors (email, Slack, etc.) |
| `event_pattern` | Matches patterns from the internal event bus |
| `mcp_notification` | Triggered by MCP server notifications |
| `schedule` | Recurring via cron expression |

## Creating a Scheduled Task in YAML

Add a task definition to your `config.yaml` or create a standalone task file:

```yaml
tasks:
  - name: morning-pr-review
    schedule:
      cron:
        expression: "0 9 * * MON-FRI"
    agent_config:
      strategy: react
      model_role: primary
      tools: [mcp:github]
    input:
      prompt: "Check open PRs in my repos and summarize anything needing review."
    data_class: INTERNAL
    retries:
      max: 2
      backoff: 5m
    timeout: 10m
```

## Managing Tasks

**From the UI:** Open the **Tasks** view in the sidebar to see all scheduled, running, and completed tasks. Click any task to view its logs, edit its schedule, or pause it.

You can also manage scheduled tasks from the **Workflows** page in the UI.

## Event-Driven Workflows

Workflows can react to events using the `incoming_message`, `event_pattern`, and `mcp_notification` trigger types:

```yaml
# workflow.yaml
name: auto-triage-issues
trigger:
  type: mcp_notification
  server: github
  method: "issues.opened"
steps:
  - name: triage
    agent_config:
      strategy: react
      tools: [mcp:github]
    input:
      prompt: "Triage this new issue: label it, estimate priority, and notify me if urgent."
```

The internal event bus connects MCP notifications, task completions, and user actions — so you can chain tasks together.

## Task Persistence & Recovery

Tasks are **durable**. The scheduler persists task state to disk, so tasks survive daemon restarts, OS reboots, and sleep/wake cycles. On startup, the scheduler detects any tasks left in a `running` state (stale from a previous crash) and automatically resets them to `pending` so they can be retried.

::: tip Resource governance
The scheduler enforces concurrency limits and per-provider rate limits. Tasks also inherit their creator's data-classification context — a task created in a `CONFIDENTIAL` session won't leak data to public channels.
:::
