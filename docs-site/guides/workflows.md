# Workflows

Workflows let you chain agents, tools, and control logic into repeatable automations. They come in two flavours: **background** (autonomous, trigger-driven) and **chat** (interactive, human-in-the-loop).

## Creating Your First Workflow

1. Open **Workflows** in the sidebar and click **New Workflow**.
2. Give it a name (e.g. `user/daily-digest`) and pick a mode — **Background** or **Chat**.
3. Add a **trigger** — what kicks the workflow off (schedule, event, or manual).
4. Add **steps** — the work the workflow actually does.
5. Hit **Run** to test it. Background workflows launch immediately; chat workflows are launched from the Chat view and attach to your conversation.

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
    outputs:
      summary: "{{result}}"
  - id: notify
    type: task
    task:
      kind: call_tool
      tool_id: comm.send_external_message
      arguments:
        channel: "#standup"
        body: "{{steps.gather.outputs.summary}}"
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
    outputs:
      result: "{{result}}"
result_message: "{{steps.setup.outputs.result}}"
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
    condition: "{{steps.analyze.outputs.lines}} > 500"
    then: [deep_review]
    else: [quick_review]
```

### For Each

```yaml
- id: process_files
  type: control_flow
  control:
    kind: for_each
    collection: "{{steps.list.outputs.files}}"
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
- **`{{steps.<id>.outputs.<field>}}`** — a named output from a completed step.
- **`{{trigger.input.<field>}}`** — an input value from the trigger (manual triggers).
- **`{{trigger.<field>}}`** — trigger data (e.g., `{{trigger.from}}`, `{{trigger.body}}` for incoming messages).
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
        value: "{{steps.gather.outputs.summary}}"
        operation: set
```

The `operation` field supports `set` (overwrite), `append_list` (add to an array), and `merge_map` (shallow-merge into an object).

::: tip
Keep workflows focused on orchestration. Put complex logic inside agent prompts or dedicated tools — workflows are the glue that connects them.
:::

## Launching Workflows

Once you've created a workflow, there are several ways to launch it depending on the trigger type and mode.

### Manual Launch (UI)

For workflows with a `manual` trigger:

1. Open the **Workflows** page from the sidebar
2. Find your workflow and click **Launch**
3. If the workflow defines an `input_schema`, a form appears where you fill in the required fields
4. Click **Run** — the workflow starts immediately

**Background workflows** run independently — the instance appears on the Workflows page where you can track its progress. **Chat workflows** are launched from the **Chat view** — they attach to your conversation and interact with you inline.

### Automatic Triggers

Workflows with non-manual triggers activate automatically once saved:

| Trigger | When it fires |
|---------|---------------|
| `schedule` | At the next matching cron time (e.g., `"0 9 * * 1-5"` fires weekdays at 9 AM) |
| `event_pattern` | When a matching event is published on the internal event bus |
| `incoming_message` | When a message arrives on the specified connector channel |
| `mcp_notification` | When a connected MCP server sends a notification |

You can **pause triggers** on any workflow without deleting it — the workflow stays saved but won't fire until you resume triggers. Toggle this from the workflow's detail panel.

### Launching from Within a Workflow

Use the `launch_workflow` step kind to start one workflow from another:

```yaml
- id: run_subreport
  type: task
  task:
    kind: launch_workflow
    workflow_name: user/generate-report
    inputs:
      date_range: "{{variables.date_range}}"
```

This is how you compose small, focused workflows into larger automations — each workflow handles one concern.

### Launching Chat Workflows

Chat workflows are launched from the **Chat view**, not the Workflows page. When you launch one:

1. Open the **Chat view** and start or select a conversation
2. Launch the chat workflow — it attaches to the conversation
3. Agent steps produce messages in the conversation thread
4. `feedback_gate` steps pause execution and present you with choices or a text input
5. Your response feeds back into the workflow, and execution continues

This makes chat workflows ideal for guided processes — onboarding, approval flows, interactive research — where the user needs to participate at key moments.

## Managing Running Workflows

Every workflow launch creates an **instance** — a running copy of the workflow definition with its own state, variables, and progress.

### Monitoring

Open the **Workflows** page to see all active and completed instances:

- **Status** — Running, Paused, Waiting (at a gate), Completed, or Failed
- **Step progress** — see which step is currently executing and review outputs from completed steps
- **Live updates** — the page updates in real time as steps complete

### Responding to Gates

When a running workflow reaches a `feedback_gate`, it pauses and waits for your input. In **chat workflows**, the gate appears as a message in your conversation. For **background workflows**, the gate surfaces on the Workflows page as a pending action.

When a workflow hits an `event_gate`, it waits for the specified event. If you configured a timeout and it expires, the step completes with a timeout payload (`error: "event_gate_timeout"`) — you can branch on this in a subsequent step to handle the timeout gracefully.

### Pause, Resume, and Kill

From the workflow instance detail panel:

- **Pause** — temporarily suspend execution. The workflow keeps its state and can be resumed later.
- **Resume** — continue a paused workflow from where it left off.
- **Kill** — immediately terminate the workflow. This cannot be undone.

### Archiving

Completed or failed instances can be **archived** to keep your Workflows page clean. Archived instances are hidden from the default view but can still be reviewed.

## Bundled Workflows

HiveMind OS ships with several ready-to-use workflows. You can launch them directly, or copy and customize them to fit your needs.

### Browsing Bundled Workflows

Open **Workflows** in the sidebar — bundled workflows appear alongside your custom workflows with a `system/` prefix. Click any workflow to view its definition, then:

- **Launch** — run it immediately with the default or your own inputs
- **Copy** — create a new workflow using **New Workflow → Copy from existing** to get an editable copy under your `user/` namespace

### Available Bundled Workflows

| Workflow | ID | Mode | What it does |
|----------|-----|------|-------------|
| **Approval Workflow** | `system/approval-workflow` | Chat | Submit a request with a title, description, and urgency. An AI agent analyzes it, then a feedback gate lets you approve, request changes, or reject. Demonstrates branching based on user decisions. |
| **Email Responder** | `system/email-responder` | Background | Auto-replies to incoming customer emails using a support agent persona with access to uploaded product documentation. |
| **Email Triage** | `system/email-triage` | Background | Classifies and routes incoming emails by intent — product questions, bug reports, billing issues — and takes appropriate action for each category. |
| **Plan and Implement** | `system/software/plan-and-implement` | Chat | A two-phase workflow: first an AI agent creates a plan, then (after your approval via a feedback gate) another agent implements it. Includes a review loop. |
| **Software Feature** | `system/software/major-feature` | Chat | Full software development lifecycle — optional spec writing, technical research/POC, planning, implementation, and documentation. Each phase has a feedback gate for human review, with `while` loops that let you request revisions. |
| **3D Print Design** | `system/3d-print/design` | Chat | Guides a 3D print CAD design workflow using specialized personas for modeling and analysis. |

::: tip Start with Approval Workflow
The **Approval Workflow** is the simplest bundled workflow and a great way to see feedback gates, branching, and variables in action. Launch it from the Workflows page to try it out.
:::

## Creating a Custom Workflow from Scratch

This walkthrough takes you from an idea to a running workflow. We'll build a **support ticket triage** workflow that classifies incoming messages and routes them to the right team.

### Step 1: Define Your Use Case

Before opening the editor, decide:

- **What triggers the workflow?** → An incoming message on the support channel
- **What should happen?** → Classify the message, then route it
- **Does a human need to be involved?** → Not for classification, but yes for edge cases
- **Background or chat?** → Background — this should run automatically

### Step 2: Create the Workflow

1. Open **Workflows → New Workflow**
2. Name it `user/support-triage`
3. Set mode to **Background**

### Step 3: Add the Trigger

Start with the incoming message trigger:

```yaml
name: user/support-triage
mode: background

steps:
  - id: trigger
    type: trigger
    trigger:
      type: incoming_message
      channel_id: support-inbox
      ignore_replies: true
    next: [classify]
```

### Step 4: Add Classification

::: v-pre
Use an `invoke_agent` step to classify the message. Note how trigger data for incoming messages is accessed directly as `{{trigger.from}}`, `{{trigger.subject}}`, `{{trigger.body}}`, etc.
:::

```yaml
  - id: classify
    type: task
    task:
      kind: invoke_agent
      persona_id: user/support-classifier
      task: |
        Classify this support message into one category:
        - bug_report
        - feature_request
        - billing
        - general_question

        From: {{trigger.from}}
        Subject: {{trigger.subject}}
        Body: {{trigger.body}}

        Return a JSON object with:
        - "category": one of the above categories
      timeout_secs: 60
    outputs:
      category: "{{result.category}}"
    on_error:
      strategy: skip
      default_output:
        category: "general_question"
    next: [route]
```

### Step 5: Add Routing with Branches

Use a `branch` step to handle each category differently:

```yaml
  - id: route
    type: control_flow
    control:
      kind: branch
      condition: "{{steps.classify.outputs.category}} == billing"
      then: [forward_to_billing]
      else: [auto_respond]
```

### Step 6: Add the Action Steps

```yaml
  - id: forward_to_billing
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        channel_id: billing-team
        to: "{{trigger.from}}"
        subject: "Billing inquiry: {{trigger.subject}}"
        body: "Forwarded billing inquiry from {{trigger.from}}:\n\n{{trigger.body}}"
    next: [end]

  - id: auto_respond
    type: task
    task:
      kind: invoke_agent
      persona_id: user/support-agent
      task: |
        Reply to this {{steps.classify.outputs.category}} message:
        From: {{trigger.from}}
        Subject: {{trigger.subject}}
        Body: {{trigger.body}}

        Write a helpful response. Return ONLY the reply text.
      timeout_secs: 120
    outputs:
      reply: "{{result}}"
    next: [send_reply]

  - id: send_reply
    type: task
    task:
      kind: call_tool
      tool_id: connector.send_message
      arguments:
        channel_id: support-inbox
        to: "{{trigger.from}}"
        subject: "Re: {{trigger.subject}}"
        body: "{{steps.auto_respond.outputs.reply}}"
    on_error:
      strategy: retry
      max_retries: 3
      delay_secs: 5
    next: [end]

  - id: end
    type: control_flow
    control:
      kind: end_workflow
```

### Step 7: Test It

1. Save the workflow
2. To test before real messages arrive, click **Launch** from the Workflows page — you'll be prompted to provide test input as JSON (e.g., `{"from": "test@example.com", "subject": "Billing help", "body": "I need an invoice"}`)
3. Watch the instance execute on the Workflows page — click into it to see step-by-step progress
4. Check that the classification is correct and the response makes sense

### Step 8: Activate

Once you're happy with the results, the workflow will automatically fire on new incoming messages on the `support-inbox` channel. You can pause triggers at any time without deleting the workflow.

::: tip Iterate with AI Assist
Use the **AI Assist** panel in the workflow editor to refine your workflow. Describe what you want to change in natural language — "add a feedback gate before sending billing inquiries" — and HiveMind OS will update the YAML for you.
:::

## Next Steps

- [Workflows Concept](/concepts/workflows) — Architecture and data flow model
- [Email Support Workflow](/examples/pr-review-workflow) — Full end-to-end email automation example
- [Onboarding Chat Workflow](/examples/chat-workflow-onboarding) — Interactive chat workflow with feedback gates
- [Daily Automation](/examples/daily-automation) — Scheduled background workflow recipes
- [Security Policies](/guides/security-policies) — Data classification and tool approval for workflow agents
