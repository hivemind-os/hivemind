# Agentic Loops Guide

This guide covers how to choose and configure agentic loop strategies in HiveMind OS. For background on the loop architecture, see [Concepts → Agentic Loops](/concepts/agentic-loops).

## Loop Strategies

HiveMind OS ships with four loop strategies. Pick the one that fits your task:

| Strategy | Best For | How It Works |
|---|---|---|
| **React** (default) | Most interactive tasks | Reason → Act → Observe cycle. The agent thinks, calls a tool, reads the result, and repeats. |
| **Sequential** | Simple linear tasks | Executes steps one after another without iterative reasoning. Lightweight and fast for straightforward work. |
| **Plan-then-Execute** | Complex multi-step work | Generates a full plan up front, then executes each step in order. Good for DevOps runbooks and research. |
| **CodeAct** | Scripting and data tasks | The agent writes Python code to take actions. Tools are exposed as Python functions in a sandboxed WASM runtime. |

::: tip
Start with **React**. Only switch strategies when you notice the agent struggling — for simple linear tasks use **Sequential**, for complex multi-step work use **Plan-then-Execute**, and for data work or scripting use **CodeAct**.
:::

## Configuring Loop Strategy per Persona

Set the `loop_strategy` field in any persona definition:

```yaml
# persona.yaml
id: user/code-reviewer
name: Code Reviewer
loop_strategy: react
preferred_models:
  - claude-sonnet
allowed_tools:
  - filesystem.read
  - filesystem.search
```

You can also override the strategy per conversation from the **persona picker → Advanced → Loop Strategy** dropdown.

Valid values for `loop_strategy` are: `react`, `sequential`, `plan_then_execute`, `code_act`.

## Choosing a Strategy

### React (default)

The React loop implements Reason-Act-Observe cycling. Each iteration the agent:

1. **Reasons** about the current state and what to do next
2. **Acts** by calling a tool or generating a response
3. **Observes** the result and decides whether to continue

This is the most flexible strategy — use it for open-ended tasks, debugging, research, and any work where the agent needs to adapt on the fly.

### Sequential

The Sequential strategy executes a fixed sequence of steps without the iterative reasoning overhead. It's best for simple, predictable tasks where the agent doesn't need to re-evaluate after each step.

### Plan-then-Execute

This strategy separates planning from execution:

1. **Plan phase** — the agent analyzes the task and generates a complete plan with discrete steps
2. **Execute phase** — each step is executed in order

This works well for complex tasks with clear sub-goals, like multi-file refactors, deployment runbooks, or research reports.

### CodeAct

The CodeAct strategy replaces structured tool calls with **Python code execution**. Instead of calling tools one at a time through the model's tool-call mechanism, the agent writes Python code in fenced blocks that runs in a sandboxed WebAssembly runtime.

Tools are exposed as pre-loaded Python functions. For example, an HTTP tool becomes `http_request(url, method)` that the agent calls directly in code. The agent can compose multiple tool calls, process results with Python logic, and build up complex workflows — all in a single code block.

```yaml
# persona.yaml — a CodeAct data analyst
id: user/data-analyst
name: Data Analyst
loop_strategy: code_act
preferred_models:
  - claude-sonnet
code_act:
  allow_network: false
  workspace_path: /workspace
```

**Tool classification:** CodeAct automatically classifies each tool:

- **Bridged** — Exposed as Python functions (HTTP, MCP tools, messaging, etc.)
- **Native** — Used as structured tool calls alongside code (`ask_user`, `delegate_task`)
- **Excluded** — Handled by Python directly (file I/O, regex, math, datetime)

**Persistent state:** Variables, imports, and state persist across code blocks within a conversation, so the agent can build on previous results.

**Sandboxed execution:** Code runs in a WebAssembly Python runtime with access only to the `/workspace` directory. Standard Python libraries are available (json, os, pathlib, re, math, csv, etc.).

## Middleware Pipeline

Every loop runs through a middleware stack with four hooks. Add middleware in the persona config:

```yaml
agent_loop:
  strategy: react
  middleware:
    - audit_logger           # Logs every model call and tool invocation
    - classification_gate    # Blocks data from crossing classification boundaries
    - approval_gate          # Pauses for human approval before sensitive actions
```

Each middleware implements the `LoopMiddleware` trait with four hooks:

| Hook | When It Runs |
|---|---|
| `before_model_call` | Before each LLM request — inject context, redact data, or block |
| `after_model_response` | After each LLM reply — log, validate, or transform |
| `before_tool_call` | Before each tool invocation — enforce policies, request approval |
| `after_tool_result` | After each tool result — filter, classify, or audit |

**Example — approval gate middleware** that pauses before write operations:

```yaml
# In tool_policy (works with approval_gate middleware)
tool_policy:
  auto_approve:
    - filesystem.read
    - filesystem.search
  require_confirmation:
    - filesystem.write
    - shell.execute
```

## Next Steps

- [Personas Guide](/guides/personas) — Configure personas that use these loop strategies
- [Workflows Guide](/guides/workflows) — Chain loops into multi-step workflows
- [Security Policies Guide](/guides/security-policies) — Set up classification gates and tool policies
