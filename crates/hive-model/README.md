# hive-model

Model routing, provider adapters, and multi-provider LLM orchestration for
[HiveMind OS](../../README.md) — a cross-platform, privacy-aware desktop AI agent.

## Architecture

`hive-model` sits between callers that need LLM completions and the concrete
providers that fulfil them. A **`ModelRouter`** accepts a `RoutingRequest`,
evaluates role bindings, capability requirements, and data-classification
constraints, then returns a `RoutingDecision` with a selected model and an
ordered fallback chain. The caller hands the decision to a **`ModelProvider`**
which executes the actual completion.

```text
RoutingRequest ──► ModelRouter ──► RoutingDecision
                                        │
                       CompletionRequest │
                                        ▼
                                  ModelProvider
                                        │
                                        ▼
                              CompletionResponse
```

## Key Types

| Type | Purpose |
|---|---|
| `ModelSelection` | Provider ID + model name pair that uniquely identifies a model. |
| `RoleBinding` | Maps a provider/model pair to a role in the router configuration. |
| `RoutingRequest` | Prompt, `DataClass`, required capabilities, role hint, and optional preferred model. |
| `RoutingDecision` | The selected `ModelSelection`, fallback chain, resolved role, and human-readable reason. |
| `CompletionRequest` | API contract sent to a provider — prompt, data class, capabilities, role, preferred model. |
| `CompletionResponse` | Provider ID, model name, and generated content returned from a completion call. |
| `ModelProvider` (trait) | Interface every provider must implement: `descriptor()` and `complete()`. |
| `ModelRouter` | Central router that resolves roles to models and enforces capability/classification rules. |

## Built-in Providers

### `EchoProvider`

A testing/development provider that echoes the prompt back with a configurable
prefix and a summary of the requested capabilities. Useful for integration tests
and offline development.

### `HttpProvider`

A generic OpenAI-compatible HTTP provider backed by `reqwest`'s blocking client.
Supports:

- Custom base URLs and endpoint paths
- Bearer-token and custom-header authentication (`ProviderAuth`)
- Arbitrary extra headers for vendor-specific APIs

## Role & Capability System

### Role-based binding

The router maps four built-in roles to provider/model pairs via configuration:

| Role | Typical use |
|---|---|
| `primary` | Default conversational model |
| `admin` | Administrative / orchestration tasks |
| `coding` | Code generation and editing |
| `scanner` | Fast, high-throughput scanning tasks |

### Capability matching

Each provider descriptor advertises a set of capabilities. The router only
considers providers whose capabilities are a superset of the request's
`required_capabilities`:

- **Chat** — general conversation
- **Code** — code generation / editing
- **Vision** — image understanding
- **Embedding** — vector embeddings
- **ToolUse** — function/tool calling

### Channel classification gating

Routing honours data-classification labels from `hive-classification`.
Public-class models are only offered public data; private-class models may
handle any classification. This prevents sensitive data from leaking to
external endpoints.

## Dependencies

### Workspace (internal)

| Crate | Role |
|---|---|
| `hive-core` | Core runtime types (`InferenceRuntimeKind`) |
| `hive-contracts` | Shared trait definitions (`Capability`, `ModelRole`, `ProviderDescriptor`, …) |
| `hive-classification` | `DataClass` / `ChannelClass` types used for classification gating |
| `hive-inference` | Lower-level inference primitives |

### External

| Crate | Role |
|---|---|
| `reqwest` | Blocking HTTP client used by `HttpProvider` |
| `serde` / `serde_json` | Serialization and deserialization of request/response types |
| `thiserror` | Ergonomic error type definitions (`ModelRouterError`) |
| `anyhow` | Flexible error propagation |
