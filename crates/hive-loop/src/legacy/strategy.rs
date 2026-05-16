//! Loop strategy trait, middleware trait, executor, and strategy kind enum.

use std::sync::Arc;

use hive_contracts::LoopStrategy as ConfigLoopStrategy;
use hive_model::ModelRouter;
use hive_tools::ToolResult;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

use super::interaction::UserInteractionGate;
use super::parsing::ToolCall;
use super::types::{BoxFuture, LoopContext, LoopError, LoopEvent, LoopResult};
use hive_model::{CompletionRequest, CompletionResponse};

// ── Middleware trait ───────────────────────────────────────────────────────

pub trait LoopMiddleware: Send + Sync {
    fn before_model_call(
        &self,
        _context: &LoopContext,
        request: CompletionRequest,
    ) -> Result<CompletionRequest, LoopError> {
        Ok(request)
    }

    fn after_model_response(
        &self,
        _context: &LoopContext,
        response: CompletionResponse,
    ) -> Result<CompletionResponse, LoopError> {
        Ok(response)
    }

    fn before_tool_call(
        &self,
        _context: &LoopContext,
        call: ToolCall,
    ) -> Result<ToolCall, LoopError> {
        Ok(call)
    }

    fn after_tool_result(
        &self,
        _context: &LoopContext,
        _tool_id: &str,
        _tool_input: Option<&serde_json::Value>,
        result: ToolResult,
    ) -> Result<ToolResult, LoopError> {
        Ok(result)
    }

    /// Drain any events generated during `before_model_call`.
    /// Called by the strategy after running the middleware chain.
    fn drain_pending_events(&self) -> Vec<LoopEvent> {
        Vec::new()
    }
}

// ── Strategy trait ────────────────────────────────────────────────────────

pub trait LoopStrategy: Send + Sync {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> BoxFuture<'a, Result<LoopResult, LoopError>>;
}

// ── Strategy kind enum ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyKind {
    ReAct,
    Sequential,
    PlanThenExecute,
    CodeAct,
}

impl StrategyKind {
    pub fn build(&self) -> Arc<dyn LoopStrategy> {
        match self {
            StrategyKind::ReAct => Arc::new(super::ReActStrategy),
            StrategyKind::Sequential => Arc::new(super::SequentialStrategy),
            StrategyKind::PlanThenExecute => Arc::new(super::PlanThenExecuteStrategy),
            StrategyKind::CodeAct => Arc::new(super::CodeActStrategy),
        }
    }
}

// ── Loop executor ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct LoopExecutor {
    strategy: Arc<dyn LoopStrategy>,
    middleware: Vec<Arc<dyn LoopMiddleware>>,
}

impl LoopExecutor {
    pub fn new(strategy: Arc<dyn LoopStrategy>) -> Self {
        Self { strategy, middleware: Vec::new() }
    }

    pub fn with_middleware(mut self, middleware: Vec<Arc<dyn LoopMiddleware>>) -> Self {
        self.middleware = middleware;
        self
    }

    fn strategy_for_context(&self, context: &LoopContext) -> Arc<dyn LoopStrategy> {
        match context.routing.loop_strategy.as_ref() {
            Some(ConfigLoopStrategy::React) => Arc::new(super::ReActStrategy),
            Some(ConfigLoopStrategy::Sequential) => Arc::new(super::SequentialStrategy),
            Some(ConfigLoopStrategy::PlanThenExecute) => Arc::new(super::PlanThenExecuteStrategy),
            Some(ConfigLoopStrategy::CodeAct) => Arc::new(super::CodeActStrategy),
            None => Arc::clone(&self.strategy),
        }
    }

    pub async fn run(
        &self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
    ) -> Result<LoopResult, LoopError> {
        self.strategy_for_context(&context)
            .run(context, model_router, &self.middleware, None, None)
            .await
    }

    pub async fn run_with_events(
        &self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
        event_tx: Sender<LoopEvent>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> Result<LoopResult, LoopError> {
        self.strategy_for_context(&context)
            .run(context, model_router, &self.middleware, Some(event_tx), interaction_gate)
            .await
    }

    pub async fn call_tool(
        &self,
        context: &LoopContext,
        tool_id: &str,
        input: Value,
    ) -> Result<ToolResult, LoopError> {
        let result = super::execute_tool_call(
            context,
            ToolCall { tool_id: tool_id.to_string(), input },
            &self.middleware,
            None,
            None,
            None,
        )
        .await?;
        Ok(result)
    }
}
