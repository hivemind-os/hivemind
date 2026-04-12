# Workflow Engine — Design Notes

This document records deliberate deviations from the original specification
(`SPEC.md`) and outlines potential paths forward.

## 1. No Event Sourcing / Replay

**Spec:** Describes event-sourced execution with append-only event logs,
checkpoint snapshots, and deterministic replay on restart (SPEC.md §Execution).

**Implementation:** Uses mutable-state SQLite persistence. Recovery resets
interrupted steps to `Pending` and re-executes them. There is no event log,
no checkpoint snapshots, and no deterministic replay.

**Rationale:** Mutable state is simpler to implement and reason about. The
current recovery mechanism (reset-and-retry) is sufficient for the expected
workload where steps are generally idempotent or externally resilient.

**Path forward:** An optional event log table could be added for audit and
debugging without changing the execution model. Full event sourcing would
require significant architectural changes to the executor and is not planned.

## 2. No Sandboxed TypeScript Custom Stages

**Spec:** Describes sandboxed QuickJS execution for user-defined custom stages
with resource limits and a restricted API surface (SPEC.md §Custom Stages).

**Implementation:** Extensibility is provided via the `StepExecutor` trait,
which allows the service layer to implement arbitrary step types in Rust. There
is no embedded JavaScript runtime and no `custom_stage` step type.

**Rationale:** The trait-based approach is more performant and avoids the
complexity of embedding and sandboxing a JS runtime. The current set of built-in
step types covers the required use cases.

**Path forward:** A `QuickJsStepExecutor` adapter could be added as a separate
crate that wraps the `StepExecutor` trait, allowing user-defined logic without
modifying the core engine.

## 3. Stage Type Taxonomy Differs from Spec

**Spec defines:** `model_call`, `tool_call`, `parallel_tool_calls`,
`conditional`, `loop`, `human_input`, `memory_read/write`, `checkpoint`,
`sub_loop`, `custom_stage`, `agent_spawn`, `parallel_agent_spawn`, `terminal`.

**Implementation provides:**
- **Tasks:** `CallTool`, `InvokeAgent`, `InvokePrompt`, `SignalAgent`,
  `FeedbackGate`, `EventGate`, `LaunchWorkflow`, `Delay`, `SetVariable`,
  `ScheduleTask`
- **Control flow:** `Branch`, `ForEach`, `While`, `EndWorkflow`

**Mapping:** Most spec concepts have implementation equivalents:
| Spec concept | Implementation |
|---|---|
| `tool_call` | `CallTool` |
| `agent_spawn` | `InvokeAgent` (sync/async) |
| `human_input` | `FeedbackGate` |
| `conditional` | `Branch` |
| `loop` | `ForEach` / `While` |
| `terminal` | `EndWorkflow` |

The implementation adds concepts not in the spec (`EventGate`, `Delay`,
`SetVariable`, `ScheduleTask`, `LaunchWorkflow`) that emerged from real-world
usage.
