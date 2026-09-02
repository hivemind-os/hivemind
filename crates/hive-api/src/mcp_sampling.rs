//! Concrete [`SamplingHandler`] implementation backed by the model router.
//!
//! This wires the MCP `sampling/createMessage` flow into the HiveMind model
//! routing subsystem.  Each request is validated against the persona's
//! sampling policy (enabled flag + max token cap + rate limit) and then
//! dispatched to the model router.
//!
//! When `mcp_sampling_requires_approval` is enabled on the persona, requests
//! are parked until a user explicitly approves or denies them (or a timeout
//! expires).

use arc_swap::ArcSwap;
use hive_contracts::Persona;
use hive_core::EventBus;
use hive_mcp::{
    CreateMessageRequestParam, CreateMessageResult, McpContent, McpError, McpRawContent,
    McpResourceContents, McpRole, SamplingHandler, SamplingMessage,
};
use hive_model::{CompletionMessage, CompletionRequest, ContentPart, FinishReason, ModelRouter};
use parking_lot::Mutex;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Default max tokens when the persona doesn't specify one.
const DEFAULT_MAX_SAMPLING_TOKENS: u32 = 16_384;

/// Default rate limit: max requests per minute per server.
const DEFAULT_RATE_LIMIT_PER_MIN: u32 = 30;

/// Default burst capacity (requests allowed in a burst before rate limiting kicks in).
const DEFAULT_RATE_BURST: u32 = 5;

/// How long to wait for user approval before auto-denying (seconds).
const APPROVAL_TIMEOUT_SECS: u64 = 30;

/// Maximum number of pending approval requests to avoid memory pressure.
const MAX_PENDING_APPROVALS: usize = 10;

/// A pending sampling approval waiting for user response.
#[derive(Debug)]
pub struct PendingSamplingApproval {
    pub id: String,
    pub server_id: String,
    pub message_count: usize,
    pub max_tokens: u32,
    pub model_hint: Option<String>,
    /// Truncated preview of the first user message for display.
    pub preview: String,
    pub created_at: Instant,
    sender: oneshot::Sender<bool>,
}

/// Serializable representation of a pending approval for the API/UI.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PendingApprovalInfo {
    pub id: String,
    pub server_id: String,
    pub message_count: usize,
    pub max_tokens: u32,
    pub model_hint: Option<String>,
    pub preview: String,
    pub remaining_secs: u64,
}

/// Maps MCP server IDs to sampling policy extracted from personas.
#[derive(Debug, Clone)]
struct SamplingPolicy {
    enabled: bool,
    requires_approval: bool,
    max_tokens: u32,
    /// Persona preferred models as fallback when the server doesn't specify hints.
    preferred_models: Option<Vec<String>>,
    /// Rate limit: requests per minute.
    rate_limit_per_min: u32,
    /// Burst capacity.
    rate_burst: u32,
}

/// Simple sliding-window rate limiter tracking request timestamps.
#[derive(Debug)]
struct RateLimiter {
    /// Recent request timestamps within the last 60 seconds.
    timestamps: std::collections::VecDeque<Instant>,
    burst: u32,
    per_min: u32,
}

impl RateLimiter {
    fn new(per_min: u32, burst: u32) -> Self {
        Self { timestamps: std::collections::VecDeque::new(), burst, per_min }
    }

    /// Returns `true` if the request should be allowed, `false` if rate-limited.
    fn allow(&mut self) -> bool {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        // Purge timestamps older than 60 seconds.
        while self.timestamps.front().is_some_and(|t| now.duration_since(*t) > window) {
            self.timestamps.pop_front();
        }

        // Check burst: count requests in the last 1 second.
        let one_sec_ago = now - std::time::Duration::from_secs(1);
        let recent_burst =
            self.timestamps.iter().rev().take_while(|t| **t >= one_sec_ago).count() as u32;
        if recent_burst >= self.burst {
            return false;
        }

        // Check sustained rate over the 60-second window.
        if self.timestamps.len() as u32 >= self.per_min {
            return false;
        }

        self.timestamps.push_back(now);
        true
    }
}

/// Concrete [`SamplingHandler`] that routes `createMessage` through the
/// model router.  Per-server policy is maintained as a mapping derived
/// from persona configs and refreshed when personas change.
pub struct ModelRouterSamplingHandler {
    model_router: Arc<ArcSwap<ModelRouter>>,
    /// server_id → policy, rebuilt each time personas are saved.
    policies: Mutex<HashMap<String, SamplingPolicy>>,
    /// Per-server rate limiters.
    rate_limiters: Mutex<HashMap<String, RateLimiter>>,
    /// Event bus for audit logging and approval events.
    event_bus: EventBus,
    /// Pending approval requests awaiting user decision.
    pending_approvals: Mutex<HashMap<String, PendingSamplingApproval>>,
}

impl ModelRouterSamplingHandler {
    pub fn new(model_router: Arc<ArcSwap<ModelRouter>>, event_bus: EventBus) -> Self {
        Self {
            model_router,
            policies: Mutex::new(HashMap::new()),
            rate_limiters: Mutex::new(HashMap::new()),
            event_bus,
            pending_approvals: Mutex::new(HashMap::new()),
        }
    }

    /// Rebuild the per-server sampling policy map from the current persona
    /// list.  Called whenever personas are created/updated/deleted.
    ///
    /// If multiple personas reference the same MCP server, the most permissive
    /// policy wins: enabled if *any* persona enables it, and the highest token
    /// cap is used.
    pub fn refresh_policies(&self, personas: &[Persona]) {
        let mut map: HashMap<String, SamplingPolicy> = HashMap::new();
        for persona in personas {
            for server in &persona.mcp_servers {
                let new_policy = SamplingPolicy {
                    enabled: persona.mcp_sampling,
                    requires_approval: persona.mcp_sampling_requires_approval,
                    max_tokens: persona
                        .mcp_sampling_max_tokens
                        .unwrap_or(DEFAULT_MAX_SAMPLING_TOKENS),
                    preferred_models: persona.preferred_models.clone(),
                    rate_limit_per_min: DEFAULT_RATE_LIMIT_PER_MIN,
                    rate_burst: DEFAULT_RATE_BURST,
                };
                map.entry(server.id.clone())
                    .and_modify(|existing| {
                        // Most permissive merge: enable if any persona enables.
                        existing.enabled = existing.enabled || new_policy.enabled;
                        existing.max_tokens = existing.max_tokens.max(new_policy.max_tokens);
                        // Approval required only if ALL personas require it.
                        existing.requires_approval =
                            existing.requires_approval && new_policy.requires_approval;
                        // Keep preferred_models from the first persona that provides them.
                        if existing.preferred_models.is_none() {
                            existing.preferred_models = new_policy.preferred_models.clone();
                        }
                    })
                    .or_insert(new_policy);
            }
        }
        // Reset rate limiters when policies change (servers may have been added/removed).
        self.rate_limiters.lock().retain(|k, _| map.contains_key(k));
        *self.policies.lock() = map;
    }

    /// Check and apply rate limiting for a given server.
    fn check_rate_limit(&self, server_id: &str, policy: &SamplingPolicy) -> Result<(), McpError> {
        let mut limiters = self.rate_limiters.lock();
        let limiter = limiters
            .entry(server_id.to_string())
            .or_insert_with(|| RateLimiter::new(policy.rate_limit_per_min, policy.rate_burst));

        if !limiter.allow() {
            tracing::warn!(server_id, "sampling request rate-limited");
            return Err(McpError::invalid_request(
                format!(
                    "rate limit exceeded ({} req/min, burst {})",
                    policy.rate_limit_per_min, policy.rate_burst,
                ),
                None,
            ));
        }
        Ok(())
    }

    /// Emit an audit event for a sampling request.
    #[allow(clippy::too_many_arguments)]
    fn emit_audit(
        &self,
        server_id: &str,
        message_count: usize,
        max_tokens: u32,
        model: Option<&str>,
        latency_ms: u128,
        success: bool,
        error_msg: Option<&str>,
    ) {
        let payload = serde_json::json!({
            "server_id": server_id,
            "message_count": message_count,
            "max_tokens": max_tokens,
            "model": model,
            "latency_ms": latency_ms,
            "success": success,
            "error": error_msg,
        });
        if let Err(e) = self.event_bus.publish("mcp.sampling", "hive-api", payload) {
            tracing::debug!(error = %e, "failed to publish mcp.sampling audit event");
        }
    }

    /// Respond to a pending sampling approval.  Returns `true` if the approval
    /// was found and resolved, `false` if it was already expired/resolved.
    pub fn respond_to_approval(&self, request_id: &str, approved: bool) -> bool {
        if let Some(pending) = self.pending_approvals.lock().remove(request_id) {
            let _ = pending.sender.send(approved);
            // Publish resolution event so the UI can dismiss the dialog.
            let _ = self.event_bus.publish(
                "mcp.sampling.approval",
                "hive-api",
                serde_json::json!({
                    "type": "resolved",
                    "id": request_id,
                    "approved": approved,
                }),
            );
            true
        } else {
            false
        }
    }

    /// List currently pending sampling approvals (for initial snapshot on connect).
    pub fn list_pending_approvals(&self) -> Vec<PendingApprovalInfo> {
        let now = Instant::now();
        let timeout = std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS);
        self.pending_approvals
            .lock()
            .values()
            .filter_map(|p| {
                let elapsed = now.duration_since(p.created_at);
                if elapsed >= timeout {
                    return None; // expired
                }
                Some(PendingApprovalInfo {
                    id: p.id.clone(),
                    server_id: p.server_id.clone(),
                    message_count: p.message_count,
                    max_tokens: p.max_tokens,
                    model_hint: p.model_hint.clone(),
                    preview: p.preview.clone(),
                    remaining_secs: (timeout - elapsed).as_secs(),
                })
            })
            .collect()
    }

    /// Create a pending approval and return a receiver to await the decision.
    fn request_approval(
        &self,
        server_id: &str,
        params: &CreateMessageRequestParam,
    ) -> Result<(String, oneshot::Receiver<bool>), McpError> {
        let mut pending = self.pending_approvals.lock();

        // Enforce cap to prevent memory/UX DoS.
        if pending.len() >= MAX_PENDING_APPROVALS {
            return Err(McpError::invalid_request(
                "too many pending sampling approvals; try again later",
                None,
            ));
        }

        let id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();

        // Build a truncated preview of the first user message.
        let preview = params
            .messages
            .first()
            .map(|m| match &m.content.raw {
                McpRawContent::Text(t) => {
                    let text = &t.text;
                    if text.len() > 200 {
                        format!("{}…", &text[..200])
                    } else {
                        text.clone()
                    }
                }
                McpRawContent::Image(_) => "[image]".to_string(),
                McpRawContent::Resource(_) => "[resource]".to_string(),
            })
            .unwrap_or_default();

        let model_hint = params
            .model_preferences
            .as_ref()
            .and_then(|p| p.hints.as_ref())
            .and_then(|hints| hints.first())
            .and_then(|h| h.name.clone());

        // Insert BEFORE broadcasting the event (per design critique).
        pending.insert(
            id.clone(),
            PendingSamplingApproval {
                id: id.clone(),
                server_id: server_id.to_string(),
                message_count: params.messages.len(),
                max_tokens: params.max_tokens,
                model_hint: model_hint.clone(),
                preview: preview.clone(),
                created_at: Instant::now(),
                sender: tx,
            },
        );
        drop(pending); // release lock before publishing

        // Broadcast the approval request event.
        let _ = self.event_bus.publish(
            "mcp.sampling.approval",
            "hive-api",
            serde_json::json!({
                "type": "requested",
                "id": id,
                "server_id": server_id,
                "message_count": params.messages.len(),
                "max_tokens": params.max_tokens,
                "model_hint": model_hint,
                "preview": preview,
                "timeout_secs": APPROVAL_TIMEOUT_SECS,
            }),
        );

        Ok((id, rx))
    }
}

impl SamplingHandler for ModelRouterSamplingHandler {
    fn create_message(
        &self,
        server_id: &str,
        params: CreateMessageRequestParam,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<CreateMessageResult, McpError>> + Send + '_>,
    > {
        // Look up policy (snapshot under lock, then release).
        let policy = self.policies.lock().get(server_id).cloned();
        let router = self.model_router.load_full();
        let server_id = server_id.to_string();
        let start = Instant::now();
        let message_count = params.messages.len();
        let requested_max_tokens = params.max_tokens;

        Box::pin(async move {
            // ── Policy check ─────────────────────────────────────
            let policy = policy.ok_or_else(|| {
                tracing::warn!(server_id, "sampling request from unknown server");
                self.emit_audit(
                    &server_id,
                    message_count,
                    requested_max_tokens,
                    None,
                    0,
                    false,
                    Some("unknown server"),
                );
                McpError::invalid_request("sampling is not configured for this server", None)
            })?;

            if !policy.enabled {
                tracing::debug!(server_id, "sampling disabled for server");
                self.emit_audit(
                    &server_id,
                    message_count,
                    requested_max_tokens,
                    None,
                    0,
                    false,
                    Some("disabled"),
                );
                return Err(McpError::invalid_request(
                    "sampling is disabled for this server's persona",
                    None,
                ));
            }

            // ── Rate limiting ────────────────────────────────────
            self.check_rate_limit(&server_id, &policy)?;

            // ── Token cap ────────────────────────────────────────
            if params.max_tokens > policy.max_tokens {
                tracing::warn!(
                    server_id,
                    requested = params.max_tokens,
                    cap = policy.max_tokens,
                    "sampling request exceeds token cap"
                );
                self.emit_audit(
                    &server_id,
                    message_count,
                    requested_max_tokens,
                    None,
                    0,
                    false,
                    Some("token cap exceeded"),
                );
                return Err(McpError::invalid_request(
                    format!(
                        "max_tokens ({}) exceeds the configured cap ({})",
                        params.max_tokens, policy.max_tokens,
                    ),
                    None,
                ));
            }

            // ── Human-in-the-loop approval ───────────────────────
            if policy.requires_approval {
                let (req_id, rx) = self.request_approval(&server_id, &params).inspect_err(|e| {
                    self.emit_audit(
                        &server_id,
                        message_count,
                        requested_max_tokens,
                        None,
                        0,
                        false,
                        Some(&e.message),
                    );
                })?;

                let timeout = std::time::Duration::from_secs(APPROVAL_TIMEOUT_SECS);
                let approved = match tokio::time::timeout(timeout, rx).await {
                    Ok(Ok(decision)) => decision,
                    Ok(Err(_)) => {
                        // Channel closed without response (shouldn't happen normally).
                        false
                    }
                    Err(_) => {
                        // Timeout expired — auto-deny and clean up.
                        self.pending_approvals.lock().remove(&req_id);
                        let _ = self.event_bus.publish(
                            "mcp.sampling.approval",
                            "hive-api",
                            serde_json::json!({
                                "type": "expired",
                                "id": req_id,
                            }),
                        );
                        false
                    }
                };

                if !approved {
                    tracing::info!(server_id, "sampling request denied by user or timed out");
                    self.emit_audit(
                        &server_id,
                        message_count,
                        requested_max_tokens,
                        None,
                        start.elapsed().as_millis(),
                        false,
                        Some("denied by user"),
                    );
                    return Err(McpError::invalid_request(
                        "sampling request was denied by the user",
                        None,
                    ));
                }
                tracing::info!(server_id, "sampling request approved by user");
            }

            // ── Log metadata if present ──────────────────────────
            if let Some(ref metadata) = params.metadata {
                tracing::debug!(
                    server_id,
                    metadata = %metadata,
                    "sampling request metadata"
                );
            }

            // ── include_context handling ─────────────────────────
            // The MCP spec defines `includeContext` as "thisServer" or
            // "allServers" to request injection of conversation context.
            // We log the request but don't inject context here because the
            // sampling handler operates outside a session scope and has no
            // access to session message history. A future enhancement could
            // accept an optional session reference for richer context.
            if let Some(ref ctx) = params.include_context {
                tracing::debug!(
                    server_id,
                    include_context = %ctx,
                    "include_context requested (not yet injected)"
                );
            }

            // ── Build CompletionRequest ──────────────────────────
            let mut messages: Vec<CompletionMessage> = Vec::new();

            // System prompt as the first message if provided.
            if let Some(sys) = &params.system_prompt {
                messages.push(CompletionMessage::text("system", sys.clone()));
            }

            // Convert SamplingMessage list — handle multimodal content.
            let mut last_content_parts: Vec<ContentPart> = Vec::new();
            for msg in &params.messages {
                let role = match msg.role {
                    McpRole::User => "user",
                    McpRole::Assistant => "assistant",
                };
                match &msg.content.raw {
                    McpRawContent::Text(t) => {
                        messages.push(CompletionMessage::text(role, t.text.clone()));
                    }
                    McpRawContent::Image(img) => {
                        // Add as image content part; use empty text message as anchor.
                        messages.push(CompletionMessage::text(role, String::new()));
                        last_content_parts.push(ContentPart::Image {
                            media_type: img.mime_type.clone(),
                            data: img.data.clone(),
                        });
                    }
                    McpRawContent::Resource(res) => {
                        // Attempt to extract text from embedded resource.
                        let text = match &res.resource {
                            McpResourceContents::TextResourceContents { text, .. } => text.clone(),
                            _ => "[binary resource]".to_string(),
                        };
                        messages.push(CompletionMessage::text(role, text));
                    }
                }
            }

            let prompt = messages.last().map(|m| m.content.clone()).unwrap_or_default();

            // Determine preferred models: fuzzy-match server hints against available
            // models, then fall back to persona preferred_models.
            let preferred_models =
                resolve_preferred_models(&params, &policy.preferred_models, &router);

            let request = CompletionRequest {
                prompt,
                prompt_content_parts: last_content_parts,
                messages,
                required_capabilities: BTreeSet::new(),
                preferred_models,
                tools: vec![],
                temperature: params.temperature,
                stop_sequences: params.stop_sequences.clone(),
                max_tokens: Some(params.max_tokens),
            };

            // Model router's `complete()` is blocking — run on a blocking thread.
            let response = tokio::task::spawn_blocking(move || router.complete(&request))
                .await
                .map_err(|e| {
                    tracing::error!(error = %e, "sampling spawn_blocking panicked");
                    McpError::internal_error("internal error servicing sampling request", None)
                })?
                .map_err(|e| {
                    let latency = start.elapsed().as_millis();
                    tracing::error!(error = %e, server_id, "model router failed for sampling");
                    self.emit_audit(
                        &server_id,
                        message_count,
                        requested_max_tokens,
                        None,
                        latency,
                        false,
                        Some(&e.to_string()),
                    );
                    McpError::internal_error(format!("model error: {e}"), None)
                })?;

            // ── Build CreateMessageResult ─────────────────────────
            let stop_reason = match response.finish_reason {
                Some(FinishReason::Stop) => {
                    Some(CreateMessageResult::STOP_REASON_END_TURN.to_string())
                }
                Some(FinishReason::Length) => {
                    Some(CreateMessageResult::STOP_REASON_END_MAX_TOKEN.to_string())
                }
                Some(FinishReason::ToolCalls) => {
                    Some(CreateMessageResult::STOP_REASON_END_TURN.to_string())
                }
                None => {
                    // Fallback heuristic when provider doesn't report finish reason.
                    if response.content.is_empty() {
                        Some(CreateMessageResult::STOP_REASON_END_MAX_TOKEN.to_string())
                    } else {
                        Some(CreateMessageResult::STOP_REASON_END_TURN.to_string())
                    }
                }
            };

            let latency = start.elapsed().as_millis();
            self.emit_audit(
                &server_id,
                message_count,
                requested_max_tokens,
                Some(&response.model),
                latency,
                true,
                None,
            );

            // Note: The model router only returns text content today.  If a
            // future provider returns images or audio we would need to map them
            // into McpContent::image or McpContent::resource here.
            Ok(CreateMessageResult {
                model: response.model,
                stop_reason,
                message: SamplingMessage {
                    role: McpRole::Assistant,
                    content: McpContent::text(response.content),
                },
            })
        })
    }
}

/// Resolve preferred models from MCP model preferences (server hints) with
/// fuzzy matching against available models, falling back to persona preferred.
fn resolve_preferred_models(
    params: &CreateMessageRequestParam,
    persona_models: &Option<Vec<String>>,
    router: &ModelRouter,
) -> Option<Vec<String>> {
    // Try the server's model hints first.
    if let Some(prefs) = &params.model_preferences {
        if let Some(hints) = &prefs.hints {
            let hint_names: Vec<&str> = hints.iter().filter_map(|h| h.name.as_deref()).collect();
            if !hint_names.is_empty() {
                // Fuzzy-match against available models.
                let available = router.available_model_ids();
                let mut matched: Vec<String> = Vec::new();
                for hint in &hint_names {
                    let hint_lower = hint.to_lowercase();
                    // Exact match first.
                    if let Some(exact) = available.iter().find(|m| m.to_lowercase() == hint_lower) {
                        matched.push(exact.clone());
                        continue;
                    }
                    // Substring match (hint is a substring of model id).
                    if let Some(sub) =
                        available.iter().find(|m| m.to_lowercase().contains(&hint_lower))
                    {
                        matched.push(sub.clone());
                        continue;
                    }
                    // Prefix match (model starts with hint).
                    if let Some(pre) =
                        available.iter().find(|m| m.to_lowercase().starts_with(&hint_lower))
                    {
                        matched.push(pre.clone());
                        continue;
                    }
                    // No match — pass the hint name as-is and let the router handle it.
                    matched.push(hint.to_string());
                }
                if !matched.is_empty() {
                    return Some(matched);
                }
            }
        }
    }
    // Fall back to the persona's preferred models.
    persona_models.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::McpServerConfig;
    use hive_mcp::{McpContent, McpRole, SamplingMessage};

    fn make_handler() -> ModelRouterSamplingHandler {
        ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            EventBus::new(16),
        )
    }

    fn test_personas(enabled: bool) -> Vec<Persona> {
        vec![Persona {
            id: "test".into(),
            name: "Test".into(),
            description: String::new(),
            system_prompt: String::new(),
            loop_strategy: Default::default(),
            preferred_models: Some(vec!["gpt-4".into()]),
            allowed_tools: vec![],
            mcp_servers: vec![hive_contracts::McpServerConfig {
                id: "srv1".into(),
                ..Default::default()
            }],
            avatar: None,
            color: None,
            tool_execution_mode: Default::default(),
            context_map_strategy: Default::default(),
            secondary_models: None,
            archived: false,
            bundled: false,
            mcp_sampling: enabled,
            mcp_sampling_requires_approval: false,
            mcp_sampling_max_tokens: Some(1000),
            prompts: vec![],
        }]
    }

    fn simple_request(max_tokens: u32) -> CreateMessageRequestParam {
        CreateMessageRequestParam {
            messages: vec![SamplingMessage {
                role: McpRole::User,
                content: McpContent::text("hello"),
            }],
            model_preferences: None,
            system_prompt: None,
            include_context: None,
            temperature: None,
            max_tokens,
            stop_sequences: None,
            metadata: None,
        }
    }

    #[test]
    fn test_policy_disabled_rejects() {
        let handler = make_handler();
        handler.refresh_policies(&test_personas(false));

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = rt.block_on(handler.create_message("srv1", simple_request(100)));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("disabled"));
    }

    #[test]
    fn test_unknown_server_rejected() {
        let handler = make_handler();
        handler.refresh_policies(&test_personas(true));

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let result = rt.block_on(handler.create_message("unknown-srv", simple_request(100)));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("not configured"));
    }

    #[test]
    fn test_token_cap_exceeded() {
        let handler = make_handler();
        handler.refresh_policies(&test_personas(true));

        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        // Request 2000 tokens but cap is 1000.
        let result = rt.block_on(handler.create_message("srv1", simple_request(2000)));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.message.contains("exceeds"));
    }

    #[test]
    fn test_rate_limiter_allows_then_blocks() {
        let mut limiter = RateLimiter::new(5, 2);
        // First two requests should pass (burst = 2).
        assert!(limiter.allow());
        assert!(limiter.allow());
        // Third request in the same instant should be blocked by burst limit.
        assert!(!limiter.allow());
    }

    #[test]
    fn test_policy_merge_most_permissive() {
        let handler = make_handler();
        // Two personas sharing the same server — one enables sampling, one doesn't.
        let personas = vec![
            Persona {
                id: "p1".into(),
                mcp_sampling: false,
                mcp_sampling_max_tokens: Some(500),
                mcp_servers: vec![hive_contracts::McpServerConfig {
                    id: "shared".into(),
                    ..Default::default()
                }],
                ..test_personas(false)[0].clone()
            },
            Persona {
                id: "p2".into(),
                mcp_sampling: true,
                mcp_sampling_max_tokens: Some(2000),
                mcp_servers: vec![hive_contracts::McpServerConfig {
                    id: "shared".into(),
                    ..Default::default()
                }],
                ..test_personas(true)[0].clone()
            },
        ];
        handler.refresh_policies(&personas);

        let policies = handler.policies.lock();
        let policy = policies.get("shared").unwrap();
        // Should be enabled (p2 enables it).
        assert!(policy.enabled);
        // Should use the higher cap.
        assert_eq!(policy.max_tokens, 2000);
    }

    #[test]
    fn test_resolve_preferred_models_falls_back_to_persona() {
        let router = ModelRouter::new();
        let params = simple_request(100);
        let persona_models = Some(vec!["gpt-4o".to_string()]);
        let result = resolve_preferred_models(&params, &persona_models, &router);
        assert_eq!(result, Some(vec!["gpt-4o".to_string()]));
    }

    #[tokio::test]
    async fn test_approval_approve_allows_request() {
        let event_bus = EventBus::new(16);
        let handler = ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            event_bus,
        );
        // Set up a policy that requires approval.
        handler.refresh_policies(&[Persona {
            mcp_sampling: true,
            mcp_sampling_requires_approval: true,
            mcp_servers: vec![McpServerConfig { id: "srv".into(), ..Default::default() }],
            ..Persona::default_persona()
        }]);

        let params = simple_request(100);
        // Request approval (spawns the pending entry).
        let (id, rx) = handler.request_approval("srv", &params).unwrap();

        // Approve it.
        assert!(handler.respond_to_approval(&id, true));
        // Receiver should get `true`.
        assert!(rx.await.unwrap());
    }

    #[tokio::test]
    async fn test_approval_deny_returns_false() {
        let event_bus = EventBus::new(16);
        let handler = ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            event_bus,
        );
        handler.refresh_policies(&[Persona {
            mcp_sampling: true,
            mcp_sampling_requires_approval: true,
            mcp_servers: vec![McpServerConfig { id: "srv".into(), ..Default::default() }],
            ..Persona::default_persona()
        }]);

        let params = simple_request(100);
        let (id, rx) = handler.request_approval("srv", &params).unwrap();

        // Deny it.
        assert!(handler.respond_to_approval(&id, false));
        assert!(!rx.await.unwrap());
    }

    #[tokio::test]
    async fn test_approval_timeout_auto_denies() {
        let event_bus = EventBus::new(16);
        let handler = ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            event_bus,
        );
        handler.refresh_policies(&[Persona {
            mcp_sampling: true,
            mcp_sampling_requires_approval: true,
            mcp_servers: vec![McpServerConfig { id: "srv".into(), ..Default::default() }],
            ..Persona::default_persona()
        }]);

        let params = simple_request(100);
        let (id, rx) = handler.request_approval("srv", &params).unwrap();

        // Simulate timeout by dropping the pending entry (sender gets dropped → rx errors).
        handler.pending_approvals.lock().remove(&id);
        // Receiver should error (channel closed).
        assert!(rx.await.is_err());
    }

    #[test]
    fn test_approval_cap_enforced() {
        let event_bus = EventBus::new(16);
        let handler = ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            event_bus,
        );
        handler.refresh_policies(&[Persona {
            mcp_sampling: true,
            mcp_sampling_requires_approval: true,
            mcp_servers: vec![McpServerConfig { id: "srv".into(), ..Default::default() }],
            ..Persona::default_persona()
        }]);

        let params = simple_request(100);
        // Fill up to MAX_PENDING_APPROVALS.
        for _ in 0..MAX_PENDING_APPROVALS {
            handler.request_approval("srv", &params).unwrap();
        }
        // Next one should fail.
        let result = handler.request_approval("srv", &params);
        assert!(result.is_err());
    }

    #[test]
    fn test_list_pending_approvals() {
        let event_bus = EventBus::new(16);
        let handler = ModelRouterSamplingHandler::new(
            Arc::new(ArcSwap::from(Arc::new(ModelRouter::new()))),
            event_bus,
        );
        handler.refresh_policies(&[Persona {
            mcp_sampling: true,
            mcp_sampling_requires_approval: true,
            mcp_servers: vec![McpServerConfig { id: "srv".into(), ..Default::default() }],
            ..Persona::default_persona()
        }]);

        let params = simple_request(100);
        let (id, _rx) = handler.request_approval("srv", &params).unwrap();

        let pending = handler.list_pending_approvals();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);
        assert_eq!(pending[0].server_id, "srv");
        assert!(pending[0].remaining_secs > 0);
    }
}
