# hive-tools

Tool registry and built-in tool implementations for [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent. Includes an MCP tool bridge for integrating external tool servers.

## Core Abstractions

### `Tool` Trait

Every tool implements the `Tool` trait, which requires:

- **`definition()`** — Returns a JSON-schema description of the tool's inputs and outputs.
- **`execute()`** — Runs the tool with the given input and returns a `ToolResult`.

### `ToolRegistry`

A runtime registry of boxed `Tool` trait objects (dynamic dispatch):

- **`register()`** — Add a tool to the registry.
- **`get()`** — Look up a tool by name.
- **`list_definitions()`** — Enumerate all registered tool schemas.

### Result and Error Types

- **`ToolResult`** — Contains the tool output along with a `data_class` tag.
- **`ToolError`** — Variants: `ExecutionFailed`, `InvalidInput`.

## Built-in Tools

| Tool | Description |
|---|---|
| `EchoTool` | Identity / test tool — returns its input unchanged |
| `CalculatorTool` | Basic arithmetic evaluation |
| `DateTimeTool` | Returns the current date and time |
| `FileSystemReadTool` | Read file contents (sandboxed) |
| `FileSystemWriteTool` | Write file contents (sandboxed) |
| `FileSystemExistsTool` | Check whether a file or directory exists |
| `FileSystemListTool` | List directory contents |
| `FileSystemGlobTool` | Glob pattern matching over the file system |
| `FileSystemSearchTool` | Search file contents by pattern |
| `HttpRequestTool` | Perform HTTP GET/POST requests |
| `JsonTransformTool` | jq-like JSON transformations |
| `ShellCommandTool` | Execute shell subprocesses |
| `KnowledgeQueryTool` | Query the knowledge graph |

## Approval & Sandboxing Model

Each tool declares metadata that the runtime uses for policy enforcement:

- **Approval policies** — `auto`, `prompt`, or `deny` — control whether a tool invocation requires user confirmation.
- **Sandboxing hints** — `read-only`, `destructive`, `idempotent`, `open-world` — describe the side-effect profile so the agent can reason about safety.

## MCP Tool Bridge

The MCP tool bridge wraps tools exposed by external [Model Context Protocol](https://modelcontextprotocol.io/) servers and registers them in the local `ToolRegistry`, making MCP tools callable through the same `Tool` trait interface.

## Dependencies

### Workspace (internal)

| Crate | Purpose |
|---|---|
| `hive-classification` | Data classification types |
| `hive-contracts` | Shared trait and type contracts |
| `hive-mcp` | MCP client integration |

### External

| Crate | Purpose |
|---|---|
| `glob` | Glob pattern matching |
| `reqwest` | HTTP client |
| `serde` / `serde_json` | Serialization |
| `tokio` | Async runtime |
| `thiserror` | Error derive macros |
