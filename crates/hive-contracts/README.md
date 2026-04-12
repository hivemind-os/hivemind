# hive-contracts

Shared data transfer objects (DTOs) for cross-boundary communication between the daemon, API, and frontend layers of **HiveMind OS** — a cross-platform, privacy-aware desktop AI agent.

## Design Principles

- **Pure serializable types** — structs carry no runtime logic, only data.
- **Single source of truth** — every boundary-crossing type lives here so producers and consumers stay in sync.
- **API versioning friendly** — flat, derive-heavy types make it straightforward to evolve wire formats.
- **Cross-language interop** — JSON-serializable contracts can be consumed by non-Rust frontends.

## Modules

| Module | Key Types | Purpose |
|--------|-----------|---------|
| `chat` | `ChatMessage`, `ChatSessionSnapshot`, `ChatRunState`, `SendMessageRequest`, `SendMessageResponse`, `InterruptMode` | Chat session state and messaging DTOs |
| `config` | `HiveMindConfig`, `ModelProviderConfig`, `PromptInjectionConfig`, `SecurityConfig`, `LocalModelsConfig` | Application and provider configuration |
| `daemon` | `DaemonStatus`, `DaemonConfig` | Daemon lifecycle and configuration |
| `hardware` | `HardwareInfo`, `MemoryInfo`, `CpuInfo`, `GpuInfo`, `RuntimeResourceUsage` | Hardware discovery and resource monitoring |
| `mcp` | `McpServerSnapshot`, `McpToolInfo`, `McpResourceInfo`, `McpPromptInfo`, `McpNotificationEvent` | Model Context Protocol server state |
| `model_router` | `ProviderDescriptor`, `RoleBindingSnapshot`, `ModelRouterSnapshot`, `Capability` | Model routing configuration snapshots |
| `models` | `InstalledModel`, `ModelCapabilities`, `HubModelInfo`, `HubSearchResult` | Local and hub model metadata |
| `risk` | `RiskScanRecord`, `PromptInjectionReview`, `ScanDecision`, `ScanSummary` | Risk-scanning and prompt-injection review |
| `scheduler` | `ScheduledTask`, `TaskStatus`, `TaskAction` | Background task scheduling |
| `tools` | `ToolDefinition`, `ToolApproval`, `ToolAnnotations` | Tool registration and approval |

## Dependencies

### Workspace

- **hive-classification** — re-exported so consumers get classification types through this crate.

### External

- **serde / serde_json** — serialization and deserialization for all contract types.

## Role in the Workspace

`hive-contracts` sits at the boundary between HiveMind OS's internal services and its external interfaces. Backend crates produce these types; the API layer serializes them; frontends consume the resulting JSON. Keeping all DTOs in one crate prevents duplicated definitions and ensures a single, version-controlled contract surface.
