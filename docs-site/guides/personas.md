# Personas Guide

This guide walks you through creating, configuring, and managing personas in HiveMind OS. For background on what personas are and how they fit into the system, see [Concepts → Personas](/concepts/personas).

## Creating a Persona from Scratch

1. Open **Settings → Personas → New Persona**
2. Fill in identity fields: **Name**, **Description**, **Avatar** (emoji or image URL), **Color** (hex)
3. Write the **system prompt** (see [tips below](#writing-effective-system-prompts))
4. Set **model preferences** — pick a primary model and optional fallbacks
5. Scope **tool access** — select only the tools this persona needs, or `*` for full access
6. Optionally add **MCP servers** for external integrations
7. Choose a **loop strategy**: `react` (default), `sequential`, or `plan_then_execute`
8. Click **Save**. Your persona appears under the `user/` namespace.

::: info 📸 Screenshot needed
The New Persona editor showing the name, avatar, system prompt, model preferences, and tool access fields.
:::

## Creating from a Template

1. In **Settings → Personas**, click **Browse Templates**
2. Pick a template and click **Use Template** — this pre-fills every field
3. Customise the name, prompt, tools, or models to fit your use case
4. Click **Save** to create your copy under `user/`

::: info 📸 Screenshot needed
The persona template browser showing available templates to start from.
:::

::: tip
Templates are a great starting point. Even if you plan to change everything, they show you what a well-structured persona looks like.
:::

## Writing Effective System Prompts

The system prompt is the single most important field. It defines how the agent behaves.

| | Example |
|---|---|
| ✅ **Good** | *"You are a security-focused code reviewer. Check for SQL injection, XSS, and auth bypasses. Never modify files. Output findings as a Markdown checklist."* |
| ❌ **Bad** | *"You are a helpful assistant. Review code and find issues."* |

- **Be specific about the role** — "You are an expert DevOps engineer" beats "You are helpful"
- **Add constraints** — "never modify files", "always cite sources"
- **Specify output format** — "respond in Markdown tables", "use numbered steps"
- **Include domain knowledge** — mention frameworks or standards the persona should follow

## Setting Model Preferences

Each persona specifies a prioritised list of models:

- **Preferred models** — primary model(s) for conversation. The agent tries each in order; glob patterns like `claude-*` are supported.
- **Secondary models** — lighter models for background tasks (context-map generation, compaction).

```yaml
preferred_models:
  - claude-sonnet
  - gpt-4o
secondary_models:
  - claude-haiku-*
  - gpt-4.1-mini
```

::: tip
Both `snake_case` and `camelCase` keys work in YAML config files — the normalizer accepts either. These docs use `snake_case` to match the Rust struct field names.
:::

## Scoping Tool Access

Restricting tools is a security best practice. Only grant what the persona actually needs.

**Example — a "Researcher" persona** (read-only):

```yaml
allowed_tools:
  - http.request
  - filesystem.read
  - filesystem.search
  - knowledge.query
```

**Example — a "Developer" persona** (full access):

```yaml
allowed_tools:
  - "*"
```

::: warning
Using `"*"` grants access to every tool, including shell execution and file writes. Only use this for trusted, general-purpose personas. For specialist agents, always scope tools down.
:::

## Adding Prompt Templates

Prompt templates are reusable Handlebars snippets attached to a persona, invokable from chat, the Agent Stage, or workflows.

Open the persona in **Settings → Personas**, scroll to **Prompt Templates → Add Template**:

::: info 📸 Screenshot needed
The Prompt Templates section in the persona editor.
:::

```yaml
prompts:
  - id: summarize-logs
    name: Summarize Logs
    description: Parses and summarizes log output
    template: |
      Analyze these logs. Provide: error count/types,
      warnings needing attention, and recommended next steps.
      ```
      {{logs}}
      ```
    input_schema:
      type: object
      properties:
        logs:
          type: string
          description: Raw log output to analyze
      required: [logs]
```

Templates with an `input_schema` render an input form in the UI automatically.

## Archiving and Restoring

Don't need a persona right now? Archive it instead of deleting.

1. In **Settings → Personas**, find the persona and click **Archive**
2. It's hidden from listings but stays resolvable — existing bots and workflows keep working

To restore: open **Settings → Personas → Show Archived**, find the persona, and click **Restore**. Built-in `system/` personas cannot be deleted, only archived — you can reset them to factory defaults at any time.

## Managing Skills per Persona

Each persona can have its own set of skills — domain-specific knowledge packs that make the agent smarter in particular areas.

1. Open the persona in **Settings → Personas** and scroll to the **Skills** section
2. Click **Manage Skills** to open the skills dialog
3. Browse available skills, toggle them on/off for this persona
4. Enabled skills appear as pills in the persona editor

::: info 📸 Screenshot needed
The Manage Skills dialog showing available skills with toggle switches.
:::

::: tip
Skills are scoped to individual personas. Install a "Kubernetes" skill on your DevOps persona without cluttering your other personas.
:::

## Walkthrough: Build a "DevOps Troubleshooter"

Let's put it all together with a complete example:

```yaml
id: user/devops-troubleshooter
name: DevOps Troubleshooter
description: Diagnoses infrastructure and deployment issues
system_prompt: |
  You are an expert DevOps engineer specializing in troubleshooting.
  When diagnosing issues:
  1. Always check logs first
  2. Verify the deployment pipeline status
  3. Check resource utilization (CPU, memory, disk)
  4. Look for recent config changes
  Be methodical and explain your reasoning step by step.
preferred_models:
  - claude-sonnet
  - gpt-4o
secondary_models:
  - claude-haiku-*
allowed_tools:
  - shell.execute
  - filesystem.read
  - http.request
  - filesystem.search
loop_strategy: plan_then_execute
context_map_strategy: code
prompts:
  - id: incident-triage
    name: Incident Triage
    description: Walk through a structured incident triage
    template: |
      A new incident has been reported: {{description}}
      Severity: {{severity}}

      Run through the standard triage checklist:
      1. Check service health endpoints
      2. Review recent deployments
      3. Inspect error rates and latency
      4. Identify blast radius
    input_schema:
      type: object
      properties:
        description:
          type: string
        severity:
          type: string
          enum: [low, medium, high, critical]
      required: [description, severity]
```

This persona can read files, search codebases, make HTTP requests, and run shell commands — but cannot write files or modify code. The `plan_then_execute` strategy means it creates a diagnostic plan before acting.

Use it in **chat** (select from the persona picker), as a **bot** (trigger on PagerDuty alerts), or in a **workflow** (`invoke_agent` step in your incident-response pipeline).

## Next Steps

- [Agentic Loops Guide](/guides/agentic-loops) — Loop strategies in depth
- [MCP Servers Guide](/guides/mcp-servers) — Connect external tools to personas
- [Bots Guide](/guides/bots) — Wrap personas with triggers and schedules
- [Concepts → Personas](/concepts/personas) — How personas work under the hood
