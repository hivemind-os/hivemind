/// Workflow Component Catalog
///
/// Provides rich, structured metadata for all workflow components.
/// Used to generate the LLM system prompt for the workflow authoring agent.
/// Generate a comprehensive workflow authoring guide for an LLM system prompt.
///
/// This guide covers all step types, trigger types, control flow, expressions,
/// variables, error handling, and the YAML format. It is designed to give an
/// AI agent everything it needs to author valid workflow definitions.
pub fn generate_authoring_guide() -> String {
    let mut guide = String::with_capacity(16_000);
    guide.push_str(WORKFLOW_AUTHORING_GUIDE);
    guide
}

const WORKFLOW_AUTHORING_GUIDE: &str = r#"
# Workflow Authoring Reference

## Overview

Workflows are defined in YAML. A workflow is a directed graph of steps triggered by events, user actions, or schedules. Steps execute sequentially or in parallel based on edge connections.

## Top-Level Structure

```yaml
id: <uuid>               # Auto-generated unique ID (optional, auto-assigned if omitted)
name: user/my-workflow    # Required. Namespace-qualified name with at least two slash-separated
                         #   segments (e.g. "user/my-workflow", "system/code-review").
                         #   Each segment may contain letters, numbers, hyphens, and underscores.
                         #   Unique per name+version.
version: "1.0"           # Version string (default: "1.0")
description: "..."       # Optional description
mode: background         # "background" (default) or "chat"
                         #   background: runs independently, managed from Workflows page
                         #   chat: attached to a chat session, shares workspace, shows result widget

variables:                # JSON Schema for workflow variables
  type: object
  properties:
    my_var:
      type: string
      default: "hello"

steps:                    # List of step definitions
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [step_a]

  - id: step_a
    type: task
    task:
      kind: call_tool
      tool_id: some_tool
      arguments:
        arg1: "{{trigger.input_value}}"
    outputs:
      result: "{{result.data}}"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:                   # Optional workflow-level output mapping
  final_result: "{{steps.step_a.outputs.result}}"

result_message: "{{steps.step_a.outputs.result}}"  # Optional: human-readable result summary (chat mode)

permissions:              # Optional default permission rules for agents spawned by this workflow
  - tool_id: filesystem.write
    approval: ask
  - tool_id: http.request
    resource: "*.internal.com/*"
    approval: auto

requested_tools:          # Optional pre-declared tools for pre-flight approval UI
  - tool_id: http.request
    approval: auto
  - tool_id: filesystem.write
    approval: ask

attachments:              # Optional file attachments associated with the workflow
  - id: ref-doc
    filename: reference.pdf
    mime_type: application/pdf
    size_bytes: 12345
```

## Triggers

Every workflow must have at least one trigger. Trigger configuration is defined inline on the trigger step.

### manual
Started by a user or API call. Accepts input parameters.

```yaml
steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      inputs:                      # Legacy simple input list
        - name: user_request
          input_type: string
          required: true
        - name: priority
          input_type: string
          required: false
          default: "medium"
      # OR use JSON Schema (takes precedence over inputs):
      input_schema:
        type: object
        required: [user_request]
        properties:
          user_request:
            type: string
            description: "The user's request"
          priority:
            type: string
            enum: [low, medium, high]
            default: medium
    next: [first_task]
```
Trigger data: The input values are available as `{{trigger.<field_name>}}`.

### incoming_message
Fires when a message arrives on a communication channel.

```yaml
steps:
  - id: start
    type: trigger
    trigger:
      type: incoming_message
      channel_id: my_slack_channel   # Required: connector channel ID
      listen_channel_id: C12345      # Optional: specific sub-channel within the connector (e.g. Slack/Discord channel ID)
      filter: ""                     # Optional: body content filter
      from_filter: ""                # Optional: sender filter
      subject_filter: ""             # Optional: subject filter
      body_filter: ""                # Optional: body filter
      mark_as_read: false            # Mark message as read after processing
      ignore_replies: false          # When true, only new messages trigger (replies are ignored)
    next: [first_task]
```
Trigger data: `{{trigger.channel_id}}`, `{{trigger.from}}`, `{{trigger.to}}`, `{{trigger.subject}}`, `{{trigger.body}}`, `{{trigger.timestamp_ms}}`, `{{trigger.external_id}}`.

### event_pattern
Fires when an event matching a topic pattern is published on the event bus.

```yaml
steps:
  - id: start
    type: trigger
    trigger:
      type: event_pattern
      topic: "order.created"        # Event bus topic (supports wildcards)
      filter: ""                    # Optional: expression filter on payload
    next: [first_task]
```
Trigger data: The event payload fields are available as `{{trigger.<field>}}`.

Topic wildcards: `chat.session.*`, `*.completed`, `scheduler.*.completed.*`.

### mcp_notification
Fires when a connected MCP server sends a notification.

```yaml
steps:
  - id: start
    type: trigger
    trigger:
      type: mcp_notification
      server_id: my_mcp_server     # MCP server ID
      kind: ""                     # Optional: notification kind filter
    next: [first_task]
```
Trigger data: Notification payload available as `{{trigger.<field>}}`.

### schedule
Fires on a cron schedule.

```yaml
steps:
  - id: start
    type: trigger
    trigger:
      type: schedule
      cron: "0 9 * * MON-FRI"     # Cron expression
    next: [first_task]
```
Trigger data: `{{trigger.scheduled_time}}`.

## Step Types

Every step has these common fields:
```yaml
- id: unique_step_id           # Required. Unique within the workflow
  type: trigger|task|control_flow  # Step category
  outputs:                     # Optional output variable mappings
    var_name: "{{result.field}}"
  on_error:                    # Optional error handling strategy
    strategy: retry
    max_retries: 3
    delay_secs: 5
  timeout_secs: 120            # Optional per-step execution timeout in seconds
  next: [next_step_id]         # Successor step IDs
```

### Trigger Step
Contains an inline trigger definition. Every workflow starts with one or more trigger steps.
```yaml
- id: start
  type: trigger
  trigger:                       # Inline trigger definition
    type: manual
    inputs: []
  next: [first_task]
```
Outputs: The trigger data (input values, message fields, event payload, etc.) is available to all downstream steps via `{{trigger.<field>}}`.

### Task Steps

#### call_tool
Calls a registered tool by its ID with template-resolved arguments.

```yaml
- id: fetch_data
  type: task
  task:
    kind: call_tool
    tool_id: http.request        # Tool ID (use discovery tools to find available tools)
    arguments:
      method: "GET"
      url: "https://api.example.com/{{variables.endpoint}}"
      headers: '{"Authorization": "Bearer {{variables.api_token}}"}'
  outputs:
    response_body: "{{result.body}}"
    status_code: "{{result.status}}"
  next: [process]
```
**Parameters:**
- `tool_id` (required): The ID of the tool to call. Use the `workflow_author.list_available_tools` tool to discover available tool IDs.
- `arguments` (map): Key-value pairs where values are template expressions resolved at runtime.

**Outputs:** Vary by tool. The tool's output is available as `{{result.<field>}}`. Use `workflow_author.get_tool_details` to see a tool's output schema.

#### invoke_agent
Spawns an AI agent with a specific persona to work on a task.

```yaml
- id: analyze
  type: task
  task:
    kind: invoke_agent
    persona_id: analyst          # Persona ID (use discovery to list personas)
    task: "Analyze the data in {{steps.fetch.outputs.response_body}} and provide insights"
    async_exec: false            # false = wait for completion; true = fire-and-forget
    timeout_secs: 300            # Optional max execution time in seconds
    permissions:                 # Optional per-step permission overrides
      - tool_id: filesystem.read
        approval: auto
    attachments:                 # Optional: IDs of workflow-level attachments to expose to this agent
      - ref-doc
  outputs:
    analysis: "{{result}}"
  next: [report]
```
**Parameters:**
- `persona_id` (required): ID of the agent persona. Use `workflow_author.list_personas` to discover available personas.
- `task` (required): Natural language task description. Supports template expressions.
- `async_exec` (default: false): If true, the workflow continues without waiting for the agent to finish.
- `timeout_secs` (optional): Maximum execution time. Agent is killed after this.
- `permissions` (optional): Tool permission overrides for this agent.
- `attachments` (optional): List of attachment IDs (from the top-level `attachments` array) to expose to the agent.

**Outputs:** `{{result}}` contains the agent's final response text. `{{result.agent_id}}` is the ID of the spawned agent. `{{result.status}}` is `completed` (sync) or `spawned` (async).

#### invoke_prompt
Renders a persona's prompt template with parameters and invokes an agent with the rendered text. Can optionally send to an existing agent instead of spawning a new one.

```yaml
- id: review
  type: task
  task:
    kind: invoke_prompt
    persona_id: code-reviewer
    prompt_id: review-pr
    parameters:
      pr_number: "{{trigger.pr_number}}"
      repo: "{{trigger.repo}}"
      severity: "warning"
    async_exec: false
    timeout_secs: 300
  outputs:
    review_result: "{{result}}"
  next: [notify]
```
**Parameters:**
- `persona_id` (required): ID of the persona containing the prompt template.
- `prompt_id` (required): ID of the prompt template within the persona.
- `parameters` (optional): Key-value map of template parameters. Values support template expressions.
- `async_exec` (default: false): If true, the workflow continues without waiting.
- `timeout_secs` (optional): Maximum execution time.
- `permissions` (optional): Tool permission overrides for this agent.
- `target_agent_id` (optional): If set, sends the rendered prompt to an existing agent instead of spawning a new one. Supports template expressions, e.g. `{{steps.spawn_step.agent_id}}`.
- `auto_create` (default: false): When `target_agent_id` is set and the target agent is not found, automatically spawn a new agent instead of failing.

**Outputs:** When spawning a new agent: `{{result}}` contains the agent's response text, `{{result.agent_id}}` is the spawned agent ID, `{{result.status}}` is `completed` or `spawned`. When targeting an existing agent: `{{result.delivered}}` (boolean).

#### signal_agent
Sends a message to a running agent or chat session. Also accepts `kind: send_message` as an alias.

```yaml
- id: notify
  type: task
  task:
    kind: signal_agent
    target:
      type: session              # "session" or "agent"
      session_id: "{{variables.session_id}}"
    content: "Workflow completed: {{steps.analyze.outputs.analysis}}"
  next: [end]
```
**Parameters:**
- `target` (required): Either `{ type: agent, agent_id: "..." }` or `{ type: session, session_id: "..." }`.
- `content` (required): Message content. Supports template expressions.

**Outputs:** `{{result.delivered}}` (boolean).

#### feedback_gate
Pauses the workflow and asks a user for input.

```yaml
- id: approval
  type: task
  task:
    kind: feedback_gate
    prompt: "Please review: {{steps.report.outputs.summary}}"
    choices:                     # Optional predefined choices
      - "Approve"
      - "Request Changes"
      - "Reject"
    allow_freeform: true         # Allow free-text response (default: true)
  outputs:
    decision: "{{result.selected}}"
    comment: "{{result.text}}"
  next: [handle_decision]
```
**Parameters:**
- `prompt` (required): The question/instruction shown to the user. Supports templates.
- `choices` (optional): List of predefined options.
- `allow_freeform` (default: true): Whether the user can type a free-form response.

**Outputs:**
- `{{result.selected}}` — The chosen option (or the freeform text if no choices).
- `{{result.text}}` — The freeform text response.

#### event_gate
Pauses the workflow until a matching event arrives on the event bus.

```yaml
- id: wait_for_order
  type: task
  task:
    kind: event_gate
    topic: "order.completed"     # Event topic to listen for
    filter: "order_id == {{variables.order_id}}"  # Optional filter expression
    timeout_secs: 3600           # Optional timeout in seconds
  outputs:
    order_data: "{{result}}"
  next: [process_order]
```
**Parameters:**
- `topic` (required): Event bus topic to listen for. Use `workflow_author.list_event_topics` to discover topics.
- `filter` (optional): Expression to filter events.
- `timeout_secs` (optional): Maximum time to wait. Step fails on timeout.

**Outputs:** The matching event's payload, available as `{{result.<field>}}`.

#### launch_workflow
Starts a child workflow instance.

```yaml
- id: start_child
  type: task
  task:
    kind: launch_workflow
    workflow_name: team/data-pipeline  # Namespaced name of the workflow to launch
    inputs:
      source: "{{steps.fetch.outputs.url}}"
      batch_size: "100"
  outputs:
    child_id: "{{result.instance_id}}"
  next: [monitor]
```
**Parameters:**
- `workflow_name` (required): Namespace-qualified name of the workflow definition to launch (e.g. `"team/data-pipeline"`). Use `workflow_author.list_workflows` to discover available workflows.
- `inputs` (map): Input values for the child workflow's trigger. Values are template expressions.

**Outputs:**
- `{{result.instance_id}}` — The launched workflow instance ID.
- `{{result.status}}` — Initial status of the child workflow.

#### delay
Pauses execution for a specified duration.

```yaml
- id: cooldown
  type: task
  task:
    kind: delay
    duration_secs: 60            # Wait time in seconds
  next: [retry]
```
**Parameters:**
- `duration_secs` (required): Number of seconds to wait.

**Outputs:** `{{result.waited_secs}}` — Actual wait time.

#### set_variable
Modifies workflow variables during execution.

```yaml
- id: update_state
  type: task
  task:
    kind: set_variable
    assignments:
      - variable: counter
        value: "{{steps.count.outputs.total}}"
        operation: set            # set | append_list | merge_map
      - variable: processed_ids
        value: "{{steps.process.outputs.id}}"
        operation: append_list
      - variable: metadata
        value: '{"status": "done", "timestamp": "{{trigger.timestamp}}"}'
        operation: merge_map
  next: [continue]
```
**Parameters:**
- `assignments` (required, array): List of variable modifications.
  - `variable`: Name of the variable to modify.
  - `value`: Template expression for the new value.
  - `operation` (default: "set"): How to apply the value.
    - `set` — Overwrite the variable.
    - `append_list` — Append to an array (creates if missing).
    - `merge_map` — Shallow-merge into an object.

**Outputs:** The assignments are applied directly to the workflow variables bag.

#### schedule_task
Registers a scheduled task with a specific action to perform.

```yaml
- id: setup_monitoring
  type: task
  task:
    kind: schedule_task
    schedule:
      name: daily_health_check
      schedule: "0 8 * * *"      # Cron expression (empty string = one-time immediate)
      action:
        type: emit_event          # Action to perform (see action types below)
        topic: monitoring.check
        payload:
          source: workflow
  outputs:
    task_id: "{{result.task_id}}"
  next: [end]
```
**Parameters:**
- `schedule.name` (required): Unique name for the scheduled task.
- `schedule.schedule` (required): Cron expression defining when to run. Use empty string `""` for a one-time immediate task.
- `schedule.action` (required): Action to perform. Must include a `type` field. Supported action types:

**Action type `emit_event`** — Emit an event on a topic:
```yaml
action:
  type: emit_event
  topic: my.topic.name
  payload: { key: value }
```

**Action type `send_message`** — Send a message to a chat session:
```yaml
action:
  type: send_message
  session_id: "session-uuid"
  content: "Hello from scheduled task"
```

**Action type `http_webhook`** — Make an HTTP request:
```yaml
action:
  type: http_webhook
  url: "https://example.com/webhook"
  method: POST
  body: '{"key": "value"}'
  headers:                       # Optional HTTP headers
    Authorization: "Bearer token"
    Content-Type: "application/json"
```

**Action type `invoke_agent`** — Invoke an AI agent with a task:
```yaml
action:
  type: invoke_agent
  persona_id: my-persona
  task: "Perform the daily analysis"
  friendly_name: "Daily Analyst"
  timeout_secs: 300
  permissions:                   # Optional permission overrides
    - tool_id: filesystem.read
      approval: auto
```

**Action type `call_tool`** — Call a specific tool:
```yaml
action:
  type: call_tool
  tool_id: fs.read_file
  arguments:
    path: /data/status.json
```

**Action type `launch_workflow`** — Launch another workflow:
```yaml
action:
  type: launch_workflow
  definition: my-other-workflow
  version: "1.0"    # Optional, omit for latest
  inputs: { key: value }
  trigger_step_id: start  # Optional: specific trigger step to target
```

**Action type `composite_action`** — Run multiple actions in sequence:
```yaml
action:
  type: composite_action
  actions:
    - type: emit_event
      topic: step1.done
      payload: {}
    - type: invoke_agent
      persona_id: my-persona
      task: "Follow-up task"
  stop_on_failure: true          # Stop on first failure (default: false)
```

**Outputs:** `{{result.task_id}}` — ID of the created scheduled task.

## Control Flow Steps

#### branch
Conditional branching (if/else). Routes execution to different paths based on a condition.

```yaml
- id: check_priority
  type: control_flow
  control:
    kind: branch
    condition: "{{variables.priority}} == high"
    then: [urgent_path]          # Steps to execute if condition is true
    else: [normal_path]          # Steps to execute if condition is false
```
**Parameters:**
- `condition` (required): Expression that evaluates to true/false.
- `then` (array): Step IDs to execute when condition is true.
- `else` (array): Step IDs to execute when condition is false.

Note: Branch steps do NOT use `next`. They route exclusively through `then` and `else`.

#### for_each

Iterates over a collection, executing body steps sequentially for each item.

```yaml
- id: process_items
  type: control_flow
  control:
    kind: for_each
    collection: "{{steps.fetch.outputs.items}}"
    item_var: current_item       # Variable name for the current item
    body: [process_one, save_one]  # Steps to execute per item
  next: [summarize]              # Steps after the loop completes
```
**Parameters:**
- `collection` (required): Template expression that resolves to an array.
- `item_var` (required): Variable name to hold the current item (accessible as `{{variables.<item_var>}}`). The index is also available as `{{variables.<item_var>_index}}` (0-based).
- `body` (array): Step IDs to execute for each item. **All steps that are part of the loop body must be listed here**, including any steps chained via `next` within the body.

#### while
Loops while a condition is true. Body steps are executed sequentially each iteration.

```yaml
- id: retry_loop
  type: control_flow
  control:
    kind: while
    condition: "{{variables.retry_count}} < 3"
    max_iterations: 10           # Safety limit (optional)
    body: [attempt, increment]
  next: [done]
```
**Parameters:**
- `condition` (required): Expression evaluated before each iteration.
- `max_iterations` (optional): Maximum number of iterations (safety limit). Defaults to 10,000 if not specified.
- `body` (array): Step IDs to execute each iteration. **All steps that are part of the loop body must be listed here.**

#### end_workflow
Explicit terminal node. No further execution.

```yaml
- id: end
  type: control_flow
  control:
    kind: end_workflow
```

## Expression Syntax

Expressions use `{{...}}` template syntax. They can reference:

### Variable Paths
- `{{variables.my_var}}` — Workflow variable
- `{{variables.nested.field}}` — Nested object field
- `{{my_var}}` — Bare variable name (shorthand for `{{variables.my_var}}`)
- `{{trigger.field_name}}` — Trigger data field
- `{{event.field_name}}` — Alias for `{{trigger.field_name}}`
- `{{steps.step_id.outputs.field}}` — Output from a previous step
- `{{result.field}}` — Current step's raw result (used in output mappings)
- `{{error}}` — Current step's error message (used in on_error contexts)

### Mixed Templates
Expressions can be embedded in strings:
```yaml
arguments:
  message: "Hello {{variables.name}}, your order #{{steps.create.outputs.order_id}} is ready"
```

### Type Preservation
A pure template (only `{{...}}` with no surrounding text) preserves the JSON type:
- `"{{steps.count.outputs.total}}"` → preserves number type (e.g., 42)
- `"Count: {{steps.count.outputs.total}}"` → becomes string (e.g., "Count: 42")

### Condition Expressions
Used in branch conditions and while loops:

**Comparisons:**
- `{{a}} == {{b}}` — Equal
- `{{a}} != {{b}}` — Not equal
- `{{a}} < {{b}}`, `{{a}} > {{b}}` — Less/greater than
- `{{a}} <= {{b}}`, `{{a}} >= {{b}}` — Less/greater than or equal

**Logical operators:**
- `expr1 && expr2` — Logical AND
- `expr1 || expr2` — Logical OR
- `!expr` — Logical NOT

**Literals:**
- `true`, `false` — Boolean literals
- `"quoted string"` — String literals
- Numbers: `42`, `3.14`
- `null` — Null literal

**Truthiness:**
- Falsy values: `""`, `"null"`, `"false"`, `"0"`, `"0.0"`
- Everything else is truthy

## Error Handling

Each step can have an `on_error` strategy:

### fail_workflow (default)
Stop the workflow with an error.
```yaml
on_error:
  strategy: fail_workflow
  message: "Step failed: critical error"   # Optional custom message
```

### retry
Retry the step a number of times with a delay.
```yaml
on_error:
  strategy: retry
  max_retries: 3
  delay_secs: 5                 # Delay between retries (default: 5)
```

### skip
Skip the failed step and continue with optional default output.
```yaml
on_error:
  strategy: skip
  default_output:               # Optional default output value
    status: "skipped"
    data: null
```

### goto
Jump to a specific step on error.
```yaml
on_error:
  strategy: goto
  step_id: error_handler        # Step to jump to
```

## Variables

The `variables` section defines the workflow's variable bag using JSON Schema:

```yaml
variables:
  type: object
  required: [api_key]
  properties:
    api_key:
      type: string
      description: "API authentication key"
    counter:
      type: number
      default: 0
    items:
      type: array
      items:
        type: string
    config:
      type: object
      properties:
        verbose:
          type: boolean
          default: false
```

Variables are accessed via `{{variables.<name>}}` and modified via `set_variable` steps.

## Workflow Output

Optional. Maps output field names to expressions evaluated when the workflow completes:

```yaml
output:
  result: "{{steps.final.outputs.data}}"
  summary: "Processed {{variables.counter}} items"
```

## Parallel Execution

Steps execute in parallel when a step's `next` field lists multiple successors:
```yaml
- id: start
  type: trigger
  trigger:
    type: manual
    inputs: []
  next: [task_a, task_b]       # task_a and task_b run in parallel

- id: task_a
  type: task
  task: { kind: delay, duration_secs: 1 }
  next: [join]

- id: task_b
  type: task
  task: { kind: delay, duration_secs: 1 }
  next: [join]

- id: join                      # Runs after BOTH task_a and task_b complete
  type: task
  task: { kind: delay, duration_secs: 0 }
  next: [end]
```

A step with multiple predecessors (join point) waits for ALL predecessors to complete.

## Validation Rules

1. All step IDs must be unique within the workflow.
2. All references in `next`, `then`, `else`, `body` must point to existing step IDs.
3. The step graph must be acyclic (except for intentional loops in while/for_each bodies).
4. At least one trigger step must exist.
5. Each trigger step must have a valid inline trigger definition.
6. `set_variable` steps must have at least one assignment with non-empty variable and value.
7. `invoke_agent` steps may only reference attachment IDs that are defined in the top-level `attachments` array.

## Workflow Design Strategy

### Choosing the Right Step Type

- **`call_tool`**: Use for deterministic actions with known parameters — API calls, file operations, sending messages. The tool does exactly what you tell it. Best when you know the exact tool ID and arguments.
- **`invoke_agent`**: Use for open-ended reasoning tasks — analyzing data, writing content, making decisions from unstructured input. The agent can think, use tools, and produce nuanced output. More expensive and slower than `call_tool`, but handles ambiguity well.
- **`invoke_prompt`**: Use when a persona already has a pre-built prompt template for the task. Combines the power of `invoke_agent` with the consistency of a tested template.
- **`feedback_gate`**: Use whenever a human should review, approve, or provide input before the workflow continues. Essential for high-stakes actions (sending external communications, making purchases, deploying code).
- **`event_gate`**: Use to pause and wait for an external event (another workflow completing, a webhook arriving, a message being received). Good for coordination between workflows.
- **`launch_workflow`**: Use to compose workflows — delegate a well-defined sub-task to another workflow. Keeps individual workflows focused and reusable.

### Error Handling Best Practices

- **Always add `on_error: retry`** on steps that call external APIs or services (`call_tool` with HTTP, `invoke_agent`). Network failures and rate limits are common.
- **Use `on_error: skip` with `default_output`** for non-critical enrichment steps. If a step adds optional context, don't let its failure kill the whole workflow.
- **Use `on_error: fail_workflow`** for critical validation steps. If input validation fails, stop early with a clear message.
- **Use `on_error: goto`** to route errors to a dedicated error-handling step that can notify the user or clean up resources.
- **Set `timeout_secs`** on all `invoke_agent` steps. Agents can run indefinitely without a timeout.

### Variable Management

- **Declare all variables upfront** in the `variables` JSON Schema with types and defaults. This documents the workflow's state and enables validation.
- **Use `set_variable` to build state progressively** — accumulate results, track counters, store intermediate values.
- **Prefer typed variables** (`type: number`, `type: boolean`, `type: array`) over untyped raw JSON.
- **Use `for_each` with `item_var`** instead of manually indexing arrays.

### Step Naming Conventions

- Use descriptive, snake_case IDs: `fetch_customer_data`, `validate_input`, `send_notification` — not `step_1`, `step_2`.
- Group related steps with common prefixes: `email_fetch`, `email_parse`, `email_reply`.
- Name trigger steps by their purpose: `on_new_email`, `daily_schedule`, `manual_start`.

### Common Anti-Patterns to Avoid

- **Don't chain 10+ sequential `invoke_agent` steps** — use `for_each` to iterate over a collection instead.
- **Don't forget `end_workflow` terminal nodes** — every execution path should reach an `end_workflow` step.
- **Don't leave variables uninitialized** — always provide defaults in the JSON Schema.
- **Don't skip error handling on external calls** — network failures will happen.
- **Don't use `invoke_agent` for simple data transformations** — `set_variable` or `call_tool` is faster and cheaper.
- **Don't hardcode values that should be variables** — use `variables` for anything that might change between runs.

## Complete Workflow Examples

### Example 1: Email Auto-Responder with Human Approval

This workflow monitors an email channel, uses an AI agent to draft a reply, asks a human to approve it, then sends the response.

```yaml
name: user/email-auto-responder
version: "1.0"
description: "Monitor emails, draft AI responses, get human approval, then send"
mode: background

variables:
  type: object
  properties:
    response_style:
      type: string
      default: "professional and helpful"

steps:
  - id: on_new_email
    type: trigger
    trigger:
      type: incoming_message
      channel_id: my_email_connector
      ignore_replies: true
    next: [draft_reply]

  - id: draft_reply
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Draft a reply to this email.
        From: {{trigger.from}}
        Subject: {{trigger.subject}}
        Body: {{trigger.body}}

        Write a {{variables.response_style}} response. Keep it concise.
        Return ONLY the reply text, no explanations.
      timeout_secs: 120
    outputs:
      draft: "{{result}}"
    on_error:
      strategy: fail_workflow
      message: "Failed to draft reply: {{error}}"
    next: [approval]

  - id: approval
    type: task
    task:
      kind: feedback_gate
      prompt: |
        New email from {{trigger.from}} about "{{trigger.subject}}".

        Proposed reply:
        {{steps.draft_reply.outputs.draft}}
      choices:
        - "Send as-is"
        - "Skip (don't reply)"
      allow_freeform: true
    outputs:
      decision: "{{result.selected}}"
      edited_text: "{{result.text}}"
    next: [check_decision]

  - id: check_decision
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.approval.outputs.decision}} == Skip (don't reply)"
      then: [end]
      else: [send_reply]

  - id: send_reply
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        channel_id: my_email_connector
        to: "{{trigger.from}}"
        subject: "Re: {{trigger.subject}}"
        body: "{{steps.approval.outputs.edited_text}}"
    on_error:
      strategy: retry
      max_retries: 3
      delay_secs: 10
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
```

### Example 2: Scheduled Report with Data Pipeline

This workflow runs on a schedule, fetches data, processes it with an AI agent, and sends the report.

```yaml
name: user/weekly-report
version: "1.0"
description: "Generate and deliver a weekly summary report every Monday"
mode: background

variables:
  type: object
  properties:
    report_data:
      type: object
      default: {}
    item_count:
      type: number
      default: 0

steps:
  - id: weekly_schedule
    type: trigger
    trigger:
      type: schedule
      cron: "0 9 * * MON"
    next: [fetch_data]

  # Also allow manual trigger for testing
  - id: manual_start
    type: trigger
    trigger:
      type: manual
      inputs: []
    next: [fetch_data]

  - id: fetch_data
    type: task
    task:
      kind: call_tool
      tool_id: http.request
      arguments:
        method: "GET"
        url: "https://api.example.com/weekly-metrics"
    outputs:
      metrics: "{{result.body}}"
    on_error:
      strategy: retry
      max_retries: 3
      delay_secs: 30
    next: [analyze]

  - id: analyze
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Analyze this weekly metrics data and write a concise executive summary report.
        Highlight trends, anomalies, and key takeaways.

        Data: {{steps.fetch_data.outputs.metrics}}
      timeout_secs: 300
    outputs:
      report: "{{result}}"
    on_error:
      strategy: fail_workflow
      message: "Analysis agent failed: {{error}}"
    next: [deliver_report]

  - id: deliver_report
    type: task
    task:
      kind: signal_agent
      target:
        type: session
        session_id: "{{variables.session_id}}"
      content: "📊 Weekly Report\n\n{{steps.analyze.outputs.report}}"
    on_error:
      strategy: skip
      default_output:
        delivered: false
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  report: "{{steps.analyze.outputs.report}}"
```

### Example 3: Multi-Agent Research and Writing

This workflow coordinates multiple specialized agents: one researches, one writes, and a human provides feedback.

```yaml
name: user/research-and-write
version: "1.0"
description: "Multi-agent workflow: research a topic, write content, get human feedback"
mode: chat

variables:
  type: object
  properties:
    revision_count:
      type: number
      default: 0

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      input_schema:
        type: object
        required: [topic]
        properties:
          topic:
            type: string
            description: "The topic to research and write about"
          depth:
            type: string
            enum: [brief, detailed, comprehensive]
            default: detailed
    next: [research]

  - id: research
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Research the following topic thoroughly: {{trigger.topic}}
        Depth level: {{trigger.depth}}

        Gather key facts, data points, expert opinions, and relevant context.
        Return a structured research brief with sources.
      timeout_secs: 300
    outputs:
      research_brief: "{{result}}"
    on_error:
      strategy: retry
      max_retries: 2
      delay_secs: 10
    next: [write]

  - id: write
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Using the following research brief, write a well-structured article about "{{trigger.topic}}".

        Research: {{steps.research.outputs.research_brief}}

        Requirements:
        - Clear introduction, body with sections, and conclusion
        - Cite specific facts from the research
        - Engaging and professional tone
      timeout_secs: 300
    outputs:
      article: "{{result}}"
    on_error:
      strategy: fail_workflow
      message: "Writing agent failed: {{error}}"
    next: [review]

  - id: review
    type: task
    task:
      kind: feedback_gate
      prompt: |
        Here's the article about "{{trigger.topic}}":

        {{steps.write.outputs.article}}

        Would you like to approve this or request revisions?
      choices:
        - "Approve"
        - "Request revisions"
      allow_freeform: true
    outputs:
      decision: "{{result.selected}}"
      feedback: "{{result.text}}"
    next: [check_approval]

  - id: check_approval
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.review.outputs.decision}} == Approve"
      then: [end]
      else: [increment_revisions]

  - id: increment_revisions
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: revision_count
          value: "1"
          operation: set
    next: [check_revision_limit]

  - id: check_revision_limit
    type: control_flow
    control:
      kind: branch
      condition: "{{variables.revision_count}} > 3"
      then: [end]
      else: [revise]

  - id: revise
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Revise this article based on the feedback below.

        Current article: {{steps.write.outputs.article}}
        Feedback: {{steps.review.outputs.feedback}}

        Make the requested changes while maintaining quality.
      timeout_secs: 300
    outputs:
      article: "{{result}}"
    on_error:
      strategy: fail_workflow
      message: "Revision agent failed: {{error}}"
    next: [review]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  final_article: "{{steps.write.outputs.article}}"

result_message: "Article about '{{trigger.topic}}' is ready!"
```

### Example 4: Event-Driven Parallel Processing

This workflow listens for events, branches on the event type, processes items in parallel, and joins the results.

```yaml
name: user/order-processor
version: "1.0"
description: "Process incoming orders with validation, fulfillment, and notification"
mode: background

variables:
  type: object
  properties:
    order_status:
      type: string
      default: "received"

steps:
  - id: on_order
    type: trigger
    trigger:
      type: event_pattern
      topic: "order.created"
    next: [validate_order]

  - id: validate_order
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Validate this order data. Check for: valid items, reasonable quantities,
        correct pricing. Return a JSON object with "valid": true/false and "issues": [].

        Order: {{trigger.payload}}
      timeout_secs: 60
    outputs:
      is_valid: "{{result.valid}}"
      issues: "{{result.issues}}"
    on_error:
      strategy: skip
      default_output:
        is_valid: false
        issues: ["Validation failed due to error"]
    next: [check_valid]

  - id: check_valid
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.validate_order.outputs.is_valid}} == true"
      then: [fulfill, notify_customer]
      else: [notify_rejection]

  # These two steps run in PARALLEL (both listed in then)
  - id: fulfill
    type: task
    task:
      kind: call_tool
      tool_id: http.request
      arguments:
        method: "POST"
        url: "https://api.example.com/fulfillment"
        body: '{"order_id": "{{trigger.order_id}}", "items": {{trigger.items}}}'
    outputs:
      fulfillment_id: "{{result.body.fulfillment_id}}"
    on_error:
      strategy: retry
      max_retries: 3
      delay_secs: 15
    next: [update_status]

  - id: notify_customer
    type: task
    task:
      kind: signal_agent
      target:
        type: session
        session_id: "{{variables.session_id}}"
      content: "Order {{trigger.order_id}} is being processed!"
    on_error:
      strategy: skip
      default_output:
        delivered: false
    next: [update_status]

  # Join point — waits for BOTH fulfill and notify_customer
  - id: update_status
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: order_status
          value: "fulfilled"
          operation: set
    next: [end]

  - id: notify_rejection
    type: task
    task:
      kind: signal_agent
      target:
        type: session
        session_id: "{{variables.session_id}}"
      content: "Order {{trigger.order_id}} was rejected: {{steps.validate_order.outputs.issues}}"
    on_error:
      strategy: skip
      default_output:
        delivered: false
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  status: "{{variables.order_status}}"
```

### Example 5: Iterating Over a Collection

This workflow demonstrates `for_each` to process multiple items, accumulating results.

```yaml
name: user/batch-processor
version: "1.0"
description: "Process a list of items one by one, collecting results"
mode: chat

variables:
  type: object
  properties:
    results:
      type: array
      default: []

steps:
  - id: start
    type: trigger
    trigger:
      type: manual
      input_schema:
        type: object
        required: [items]
        properties:
          items:
            type: array
            items:
              type: string
            description: "List of items to process"
    next: [process_items]

  - id: process_items
    type: control_flow
    control:
      kind: for_each
      collection: "{{trigger.items}}"
      item_var: current_item
      body: [process_one, save_result]
    next: [summarize]

  - id: process_one
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: "Process this item and return a brief result: {{variables.current_item}}"
      timeout_secs: 120
    outputs:
      item_result: "{{result}}"
    on_error:
      strategy: skip
      default_output:
        item_result: "Error processing item"
    next: [save_result]

  - id: save_result
    type: task
    task:
      kind: set_variable
      assignments:
        - variable: results
          value: "{{steps.process_one.outputs.item_result}}"
          operation: append_list
    next: []

  - id: summarize
    type: task
    task:
      kind: invoke_agent
      persona_id: system/general
      task: >
        Summarize the results of processing {{trigger.items}} items:
        {{variables.results}}

        Provide a concise summary of what was processed and any notable findings.
      timeout_secs: 120
    outputs:
      summary: "{{result}}"
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow

output:
  results: "{{variables.results}}"
  summary: "{{steps.summarize.outputs.summary}}"

result_message: "{{steps.summarize.outputs.summary}}"
```
"#;
