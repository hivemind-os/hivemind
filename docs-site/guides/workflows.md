# Workflows

Workflows let you chain agents, tools, and control logic into repeatable automations. They come in two flavours: **background** (autonomous, trigger-driven) and **chat** (interactive, human-in-the-loop).

## Creating Your First Workflow

1. Open **Workflows** in the sidebar and click **New Workflow**.
2. Give it a name (e.g. `user/daily-digest`) and pick a mode — **Background** or **Chat**.
3. Add a **trigger** — what kicks the workflow off (schedule, event, or manual).
4. Add **steps** — the work the workflow actually does.
5. Hit **Run** to test it. Background workflows launch immediately; chat workflows attach to your current conversation.

## Visual Designer vs YAML Editor

HiveMind OS gives you three ways to build workflows:

- **Visual designer** — drag-and-drop step nodes onto a canvas and connect them. Great for exploring what's possible.
- **YAML editor** — write the workflow definition directly. Faster for power users and easy to version-control.
- **AI generation** — describe what you want in natural language and let HiveMind OS generate the YAML for you.

::: tip
Start in the visual designer to learn the step types, then switch to YAML once you're comfortable — the two stay in sync automatically.
:::

## Background Workflows

Background workflows run autonomously without user interaction. They're ideal for automations that should just *happen*.

### Triggers

| Trigger | Description |
|---------|-------------|
| `manual` | Triggered manually by a user (optionally with an input schema) |
| `schedule` | Cron expression (e.g. `"0 9 * * 1-5"` for weekdays at 9 AM) |
| `event_pattern` | Fires on internal event bus topics |
| `mcp_notification` | Fires when an MCP server sends a notification |
| `incoming_message` | Fires on messages from a connector (email, Slack, Discord, etc.) |

### Monitoring

Every run creates an **instance** visible on the Workflows page. From there you can inspect status, step-by-step logs, and output values in real time.

### Example: Daily Standup Summary

```yaml
name: user/daily-standup
mode: background
steps:
  - id: trigger
    type: trigger
    trigger:
      type: schedule
      cron: "0 9 * * 1-5"
  - id: gather
    type: task
    task:
      kind: invoke_agent
      persona_id: user/project-manager
      task: "Summarize yesterday's git commits and open PRs for the team"
  - id: notify
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        channel: "#standup"
        body: "{{steps.gather.output}}"
```

**More ideas:** automated code scanning, nightly report generation, data pipeline orchestration.

## Chat Workflows

Chat workflows run inside a conversation. They can pause to ask questions, present choices, and wait for approval before continuing.

### Key Capabilities

- **Feedback gates** — pause execution and ask the user to confirm or choose.
- **Interactive data gathering** — collect inputs step-by-step through the chat.
- **Result messages** — display a formatted summary when the workflow completes.

### Example: Guided Project Setup

```yaml
name: user/project-setup
mode: chat
steps:
  - id: trigger
    type: trigger
    trigger:
      type: manual
      input_schema:
        type: object
        properties:
          projectName:
            type: string
  - id: ask_stack
    type: task
    task:
      kind: invoke_agent
      persona_id: user/developer
      task: "What tech stack should we use for {{trigger.input.projectName}}?"
  - id: confirm
    type: task
    task:
      kind: feedback_gate
      prompt: "Here's my recommendation. Shall I proceed?"
  - id: setup
    type: task
    task:
      kind: invoke_agent
      persona_id: user/developer
      task: "Set up project {{trigger.input.projectName}} with the agreed stack"
result_message: "{{steps.setup.output}}"
```

## Step Types Reference

Every step has a `type` (`trigger`, `task`, or `control_flow`) and a `kind` that determines what it does.

### Task Kinds

| Kind | What it does |
|------|-------------|
| `call_tool` | Invoke any MCP tool by `tool_id` with `arguments` |
| `invoke_agent` | Spawn an agent with a persona and a task prompt |
| `invoke_prompt` | Resolve a persona's prompt template with parameters |
| `feedback_gate` | Pause and ask the user for confirmation or input (chat mode) |
| `event_gate` | Pause until a specific event arrives on a topic |
| `launch_workflow` | Start another workflow, optionally passing `inputs` |
| `schedule_task` | Register a cron-scheduled action |
| `delay` | Wait for `duration_secs` before continuing |
| `set_variable` | Assign, append, or merge values into workflow variables |
| `signal_agent` | Send a message to a running agent or session |

## Control Flow

Use `type: control_flow` steps to add branching and iteration.

### Branch

```yaml
- id: check_size
  type: control_flow
  control:
    kind: branch
    condition: "{{steps.analyze.output.lines}} > 500"
    then: [deep_review]
    else: [quick_review]
```

### For Each

```yaml
- id: process_files
  type: control_flow
  control:
    kind: for_each
    collection: "{{steps.list.output.files}}"
    item_var: current_file
    body: [lint_file]
```

### While

```yaml
- id: poll
  type: control_flow
  control:
    kind: while
    condition: "{{variables.status}} != 'ready'"
    max_iterations: 10
    body: [check_status, wait]
```

## Error Handling

Attach an `on_error` strategy to any step:

| Strategy | Behaviour |
|----------|-----------|
| `retry` | Retry up to `max_retries` times with `delay_secs` between attempts |
| `skip` | Skip the step and optionally provide a `default_output` |
| `goto` | Jump to a specific `step_id` |
| `fail_workflow` | Abort the workflow with an optional `message` |

```yaml
- id: flaky_api
  type: task
  task:
    kind: call_tool
    tool_id: fetch_data
    arguments:
      url: "https://api.example.com/data"
  on_error:
    strategy: retry
    max_retries: 3
    delay_secs: 10
```

## Variables and Data Flow

Workflows pass data between steps using **template expressions**.

<!-- prettier-ignore -->
::: v-pre
- **`{{steps.<id>.output}}`** — the output of a completed step.
- **`{{trigger.input.<field>}}`** — an input value from the trigger.
- **`{{variables.<name>}}`** — a workflow-scoped variable set by `set_variable` steps.
:::

Use a `set_variable` step to accumulate or transform data mid-workflow:

```yaml
- id: save_result
  type: task
  task:
    kind: set_variable
    assignments:
      - variable: summary
        value: "{{steps.gather.output}}"
        operation: set
```

The `operation` field supports `set` (overwrite), `append_list` (add to an array), and `merge_map` (shallow-merge into an object).

::: tip
Keep workflows focused on orchestration. Put complex logic inside agent prompts or dedicated tools — workflows are the glue that connects them.
:::
