//! hive-loop — Agentic loop engine and strategy framework for HiveMind chat sessions.
//!
//! # Overview
//!
//! This crate provides two subsystems:
//!
//! 1. **Legacy agentic loop** (`legacy/`) — the production chat loop used by `hive-chat`.
//!    `LoopExecutor` selects a strategy (ReAct, Sequential, PlanThenExecute, or CodeAct)
//!    from `legacy/strategies/`, and middleware layers handle compaction, token budgets,
//!    classification, risk scanning, and stall detection.
//!
//! 2. **Generic workflow engine** — a standalone YAML-driven workflow engine that loads
//!    definitions, executes them step-by-step against pluggable model and tool backends,
//!    and persists state for crash recovery. Note: the *product* workflow path uses
//!    `hive-workflow::WorkflowEngine`, not this engine.
//!
//! # Quick Start
//!
//! ```ignore
//! use hive_loop::{WorkflowEngine, InMemoryStore, NullEventSink};
//!
//! let engine = WorkflowEngine::new(model, tools, store, events);
//! let result = engine.run_builtin("react", run_id, inputs).await?;
//! ```

// New workflow engine modules
pub mod actions;
pub mod engine;
pub mod error;
pub mod expression;
pub mod schema;
pub mod state;
pub mod store;
pub mod traits;
pub mod workflows;

// Legacy module (will be removed after hive-api migration)
pub mod legacy;

// CodeAct code-block extraction
pub mod code_extraction;

// CodeAct system prompt construction
pub mod code_act_prompt;

// Token budget enforcement middleware
pub mod token_budget;

// Context compaction middleware (SPEC.md §9.12)
pub mod compactor;

// Risk-scanning middleware for prompt injection detection
pub mod risk_middleware;

// Data-classification enforcement middleware
pub mod classification_middleware;

// Shared tool-call policy evaluation
pub mod tool_policy;

// Stall detection for runaway agent loops
pub mod stall_detector;

// Stall detection middleware (consecutive counting + warning-before-stop)
pub mod stall_middleware;

// Adaptive tool-call budget (soft limit + auto-extend + hard ceiling)
pub mod tool_budget;

// ── New workflow engine public API ──────────────────────────────────────────
pub use actions::{ActionExecutor, ActionOutcome};
pub use engine::WorkflowEngine;
pub use error::{WorkflowError, WorkflowResult};
pub use schema::WorkflowDefinition;
pub use state::{WorkflowState, WorkflowStatus};
pub use store::{InMemoryStore, WorkflowStore};
pub use traits::{
    Message, MessageRole, ModelBackend, ModelRequest, ModelResponse, NullEventSink, ToolBackend,
    ToolCall as WfToolCall, ToolResult as WfToolResult, ToolSchema, WorkflowEvent,
    WorkflowEventSink,
};

// ── Legacy re-exports (for hive-api backward compatibility) ────────────────
pub use legacy::{
    parse_tool_call, AgentContext, AgentOrchestrator, BoxFuture, CodeActStrategy,
    CodeExecutionPhase, ConversationContext, ConversationJournal, JournalEntry, JournalPhase,
    JournalToolCall, KnowledgeQueryHandler, LoopContext, LoopError, LoopEvent, LoopExecutor,
    LoopMiddleware, LoopResult, LoopStrategy, PlanThenExecuteStrategy, ReActStrategy,
    RoutingConfig, SecurityContext, SequentialStrategy, StrategyKind, ToolCall as LegacyToolCall,
    ToolsContext, UserInteractionGate,
};

// ── Stall detection + adaptive budget ─────────────────────────────────────
pub use stall_detector::{StallDetector, StallStatus};
pub use stall_middleware::StallDetectionMiddleware;
pub use tool_budget::{AdaptiveBudget, BudgetDecision};

// ── Token budget middleware ────────────────────────────────────────────────
pub use token_budget::{estimate_request_tokens, TokenBudgetMiddleware};

// ── Context compaction middleware ──────────────────────────────────────────
pub use compactor::ContextCompactorMiddleware;

// ── Risk scanning middleware ──────────────────────────────────────────────
pub use risk_middleware::RiskScanMiddleware;

// ── Data classification middleware ──────────────────────────────────────
pub use classification_middleware::DataClassificationMiddleware;
