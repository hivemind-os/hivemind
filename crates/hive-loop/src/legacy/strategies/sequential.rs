use std::sync::Arc;

use hive_model::{CompletionRequest, RoutingRequest};

use super::super::interaction::UserInteractionGate;
use super::super::strategy::{LoopMiddleware, LoopStrategy};
use super::super::types::{BoxFuture, LoopContext, LoopError, LoopEvent, LoopResult};
use super::super::{model_router_error_to_loop_error, try_recover_context_limit};

#[derive(Default)]
pub struct SequentialStrategy;

impl LoopStrategy for SequentialStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<hive_model::ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        _event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        _interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> BoxFuture<'a, Result<LoopResult, LoopError>> {
        Box::pin(async move {
            let routing_request = RoutingRequest {
                prompt: context.conversation.prompt.clone(),
                required_capabilities: context.routing.required_capabilities.clone(),
                preferred_models: context.routing.preferred_models.clone(),
            };
            let decision = if let Some(decision) = context.routing.routing_decision.clone() {
                decision
            } else {
                model_router
                    .route(&routing_request)
                    .map_err(|error| LoopError::ModelRouting(error.to_string()))?
            };

            // Store the routing decision so middleware (e.g. compactor) can
            // look up the correct model limits.
            let mut context = context;
            context.routing.routing_decision = Some(decision.clone());

            let mut request = CompletionRequest {
                prompt: context.conversation.prompt.clone(),
                prompt_content_parts: context.conversation.prompt_content_parts.clone(),
                messages: context.conversation.history.clone(),
                required_capabilities: context.routing.required_capabilities.clone(),
                preferred_models: context.routing.preferred_models.clone(),
                tools: context.tools_ctx.tools.list_definitions(),
                temperature: None,
                stop_sequences: None,
                max_tokens: None,
            };

            for hook in middleware {
                request = hook.before_model_call(&context, request)?;
            }

            let router = Arc::clone(&model_router);
            let decision_clone = decision.clone();
            let request_clone = request.clone();
            let blocking_future = tokio::task::spawn_blocking(move || {
                router.complete_with_decision(&request_clone, &decision_clone)
            });
            let model_result = if let Some(ref token) = context.cancellation_token {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        return Err(LoopError::Cancelled);
                    }
                    result = blocking_future => {
                        result
                            .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                            .map_err(model_router_error_to_loop_error)
                    }
                }
            } else {
                blocking_future
                    .await
                    .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                    .map_err(model_router_error_to_loop_error)
            };
            let response = match model_result {
                Ok(resp) => resp,
                Err(err) => {
                    if let Some(truncated) = try_recover_context_limit(&err, &request) {
                        let router2 = Arc::clone(&model_router);
                        let decision2 = decision.clone();
                        let retry = tokio::task::spawn_blocking(move || {
                            router2.complete_with_decision(&truncated, &decision2)
                        });
                        retry
                            .await
                            .map_err(|e| LoopError::JoinFailed(e.to_string()))?
                            .map_err(model_router_error_to_loop_error)?
                    } else {
                        return Err(err);
                    }
                }
            };

            let mut response = response;
            for hook in middleware {
                response = hook.after_model_response(&context, response)?;
            }

            Ok(LoopResult {
                content: response.content,
                provider_id: response.provider_id,
                model: response.model,
                decision,
                preempted: false,
            })
        })
    }
}
