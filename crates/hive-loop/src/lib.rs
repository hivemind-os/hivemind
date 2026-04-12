//! hive-loop — A standalone YAML-driven workflow execution engine.
//!
//! # Overview
//!
//! This crate provides a generic workflow engine that loads workflow definitions
//! from YAML, executes them step-by-step against pluggable model and tool backends,
//! and persists execution state for crash recovery and resumability.
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
    parse_tool_call, AgentContext, AgentOrchestrator, BoxFuture, ConversationContext,
    ConversationJournal, JournalEntry, JournalPhase, JournalToolCall, KnowledgeQueryHandler,
    LoopContext, LoopError, LoopEvent, LoopExecutor, LoopMiddleware, LoopResult, LoopStrategy,
    PlanThenExecuteStrategy, ReActStrategy, RoutingConfig, SecurityContext, SequentialStrategy,
    StrategyKind, ToolCall as LegacyToolCall, ToolsContext, UserInteractionGate,
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
