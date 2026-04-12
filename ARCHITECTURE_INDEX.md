# HIVEMIND OS CODEBASE - ARCHITECTURE EXPLORATION COMPLETE

## Documentation Created

Two comprehensive documents have been generated in C:\dev\forge\:

1. **ARCHITECTURE_SUMMARY.md** (14.6 KB)
   - Detailed breakdown of all 5 crates
   - Middleware system with examples
   - Tool call flow diagrams
   - Configuration hotswap mechanism
   - Complete type definitions and trait signatures
   
2. **ARCHITECTURE_INDEX.md** (This file)
   - Quick reference guide
   - File manifests
   - System constants and limits
   - Integration points
   - Next steps for learning

## Quick Summary: The 5 Crates

### 1. hive-loop (2,716 lines) - Agent Reasoning Loop
- **Legacy ReAct Loop**: LoopContext, ToolCall, LoopStrategy (ReAct, Sequential, PlanThenExecute)
- **Modern Workflow Engine**: WorkflowEngine, WorkflowDefinition (YAML), ActionExecutor
- **Middleware System**: 4 hooks (before/after model call, before/after tool call)
- **Token Budget**: Truncates history/prompt to fit context window
- **Context Compaction**: Summarizes old messages to reduce tokens
- **Tool Execution**: Sequential/parallel with permission checks and user interaction

Key Files:
- legacy.rs (2,716 lines) - Full ReAct implementation
- engine.rs - Modern workflow engine
- traits.rs - Backend abstractions (ModelBackend, ToolBackend)
- token_budget.rs - Token limiting middleware  
- compactor.rs - Context summarization middleware

### 2. hive-mcp - Model Context Protocol Client
- Tool/resource/prompt discovery from external MCP servers
- Stdio (local subprocess) and SSE (HTTP) transports
- Connection status tracking per server
- Notification queue (max 200 events)
- Full service API: list_servers, connect, call_tool, list_resources, list_prompts

Key Types: McpServerSnapshot, McpToolInfo, McpResourceInfo, McpPromptInfo

### 3. hive-skills-service (342 lines) - Skills Management
- discover()  query GitHub sources
- install_skill()  parse SKILL.md, audit, extract files
- rebuild_catalog()  create SkillCatalog for agents
- uninstall_skill(), set_skill_enabled()
- Storage: {data_dir}/skills.db (SQLite), {storage_dir}/(name)/ (files)

Key Types: SkillManifest, DiscoveredSkill, InstalledSkill, SkillAuditResult

### 4. hive-contracts - Core Types
- tools.rs: ToolApproval, ToolAnnotations, ToolDefinition
- interaction.rs: UserInteraction (ToolApproval/Question), UserInteractionResponse
- chat.rs: ReasoningEvent, ConversationModality trait
- config.rs: HiveMindConfig, SecurityConfig, PolicyAction, PromptInjectionConfig
- permissions.rs: SessionPermissions, PermissionRule, infer_scope()
- skills.rs: Skill types
- mcp.rs: MCP types

### 5. hive-runtime-worker - Inference Runtime
- Isolated subprocess per runtime (candle, onnx, llama-cpp)
- IPC protocol for daemon communication
- Event loop for continuous model loading/inference
- Panic hook logs to stderr before abort

## Key Architectural Patterns

### Tool Call Pipeline
Model Response -> parse_tool_calls() -> execute_tool_batch() -> 
  [before_tool_call middleware -> permission check -> user interaction? -> 
   tool execution -> after_tool_result middleware] -> format results -> 
  append to prompt -> loop continues (ReAct) or exits

### Middleware Execution
4 interception hooks on each model call:
1. before_model_call(context, request) - Truncate history/prompt, modify prompt
2. after_model_response(context, response) - Filter tool calls, modify content
3. before_tool_call(context, call) - Log, validate, modify args
4. after_tool_result(context, result) - Transform output, add metadata

### Loop Strategies
- **ReAct**: Iterates until model returns no tool calls (limit: 25 iterations)
- **Sequential**: Single pass, no tool iterations
- **PlanThenExecute**: Parse plan first, execute each step (limit: 10 steps, 10 tools/step)

### Persistence & Resume
- ConversationJournal records all tool cycles
- reconstruct_react_prompt() rebuilds full state after crash
- WorkflowState persisted to store (SQLite, Redis, etc.)
- Resume from checkpoint by loading state + replaying

### Configuration Hotswap
- Arc<ArcSwap<Config>> enables live updates
- No agent restart needed for config changes
- Next model call uses updated configuration

## System Limits

| Constant | Value | Purpose |
|---|---|---|
| MAX_TOOL_CALLS | 25 | ReAct loop iteration limit |
| MAX_PLAN_STEPS | 10 | PlanThenExecute step limit |
| MAX_TOOL_CALLS_PER_STEP | 10 | Per-step tool limit |
| MAX_TOOL_OUTPUT_CHARS | 100,000 | Output truncation (~25K tokens) |
| KEEP_RECENT_MESSAGES | 6 | History msgs preserved during truncation |
| OUTPUT_RESERVE_FRACTION | 0.15 | Model output headroom (15% of context) |
| OUTPUT_RESERVE_MIN | 2048 | Minimum output tokens |

## For Deep Dives

Tool Execution Pipeline: legacy.rs:1164-1200 (execute_tool_call)
Middleware Pattern: legacy.rs:348-380 (LoopMiddleware trait)
Tool Parsing: legacy.rs:1748-1826 (parse_tool_calls, JSON extraction)
Persistence: legacy.rs:91-181 (ConversationJournal)
Strategy Selection: legacy.rs:920-980 (LoopStrategy implementations)
Modern Engine: engine.rs + schema.rs (YAML-driven workflows)
MCP: hive-mcp/README.md + contracts/mcp.rs
Skills: hive-skills-service/lib.rs + contracts/skills.rs

---
Exploration completed with comprehensive documentation.
