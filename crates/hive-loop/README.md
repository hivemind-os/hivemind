# hive-loop

Agentic loop engine for [HiveMind OS](../../README.md) — provides pluggable reasoning strategies and a middleware pipeline for orchestrating model calls and tool invocations.

## Overview

`hive-loop` drives the core agent loop: route a prompt to a model, parse tool calls from the response, execute tools, and feed results back until the model produces a final answer. Different **strategies** control the reasoning approach, while **middleware** hooks allow cross-cutting concerns (audit logging, rate limiting, classification checks) to be injected without modifying strategy code.

## Strategies

Strategies implement the `LoopStrategy` trait and are injected as trait objects at runtime.

| Strategy | Description |
|---|---|
| `ReActStrategy` | Reason → Act → Observe cycles. The model thinks, invokes a tool, observes the result, and repeats until it has enough information to answer. |
| `SequentialStrategy` | Executes tool calls sequentially in the order the model emits them. |
| `PlanThenExecuteStrategy` | Two-phase approach: the model first produces a plan, then executes each step. |

```text
User prompt
  │
  ▼
┌─────────────┐   tool call    ┌────────────┐
│  Model Call  │ ─────────────▶│  Tool Exec  │
└──────┬──────┘                └─────┬──────┘
       │ final answer                │ result
       ▼                             │
  LoopResult ◀───────────────────────┘
```

## Middleware Pipeline

Middleware implements the `LoopMiddleware` trait. Each middleware can intercept four points in the loop:

```
before_model_call  →  MODEL  →  after_model_response
before_tool_call   →  TOOL   →  after_tool_result
```

Any hook can inspect/transform its payload or reject it by returning a `LoopError`.

## Key Types

### `LoopContext`

Input to a strategy run. Carries the session ID, message ID, prompt text, data classification, required capabilities, optional role, routing decision, and the tool registry.

### `LoopResult`

Output of a successful run: the final content string, the provider and model that produced it, and the routing decision used.

### `LoopError`

Covers failure modes across the loop:

- `ModelRouting` / `ModelExecution` — model-layer failures
- `ToolUnavailable` / `ToolDenied` / `ToolApprovalRequired` — tool-access errors
- `ToolExecutionFailed` — a tool returned an error
- `ToolCallLimit` — configurable cap reached (default 16)
- `MiddlewareRejected` — a middleware hook vetoed the request

## Traits

| Trait | Purpose |
|---|---|
| `LoopStrategy` | `fn run(context, model_router, middleware) → BoxFuture<Result<LoopResult, LoopError>>` |
| `LoopMiddleware` | Optional hooks: `before_model_call`, `after_model_response`, `before_tool_call`, `after_tool_result` |

## Dependencies

### Workspace crates

- **hive-classification** — data classification types (`DataClass`)
- **hive-model** — model routing, completion request/response types
- **hive-tools** — tool registry, approval, and result types

### External

- `tokio` — async runtime
- `thiserror` — error derive macros
- `serde_json` — JSON value handling
- `tracing` — structured logging
- `anyhow` — error context propagation
