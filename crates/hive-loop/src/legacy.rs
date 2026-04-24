use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{
    infer_scope_with_workspace, InteractionKind, InteractionResponsePayload,
    LoopStrategy as ConfigLoopStrategy, Persona, SessionPermissions, ToolExecutionMode,
    ToolLimitsConfig, UserInteractionResponse, WorkspaceClassification,
};
use hive_model::{
    Capability, CompletionMessage, CompletionRequest, CompletionResponse, ContentPart, ModelRouter,
    ModelRouterError, RetryInfo, RoutingDecision, RoutingRequest,
};
use hive_tools::{ToolApproval, ToolRegistry, ToolResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering as AtomicOrdering};
use std::sync::Arc;

use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;

use tokio_util::sync::CancellationToken;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Metadata for a pending user interaction.
struct PendingInteraction {
    tx: oneshot::Sender<UserInteractionResponse>,
    kind: InteractionKind,
}

/// Gate that allows the ReAct loop to pause and request user interaction
/// (tool approval, questions, etc.). Transport-agnostic: any channel
/// (desktop UI, mobile push, Slack bot) can call `respond()`.
pub struct UserInteractionGate {
    pending: Mutex<HashMap<String, PendingInteraction>>,
}

impl std::fmt::Debug for UserInteractionGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let count = self.pending.lock().len();
        f.debug_struct("UserInteractionGate").field("pending_count", &count).finish()
    }
}

impl Default for UserInteractionGate {
    fn default() -> Self {
        Self::new()
    }
}

impl UserInteractionGate {
    pub fn new() -> Self {
        Self { pending: Mutex::new(HashMap::new()) }
    }

    /// Store a pending interaction request. Returns receiver to await the response.
    pub fn create_request(
        &self,
        request_id: String,
        kind: InteractionKind,
    ) -> oneshot::Receiver<UserInteractionResponse> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(request_id, PendingInteraction { tx, kind });
        rx
    }

    /// Returns the interaction kind for a pending request, if it exists.
    pub fn get_pending_kind(&self, request_id: &str) -> Option<InteractionKind> {
        let pending = self.pending.lock();
        pending.get(request_id).map(|p| p.kind.clone())
    }

    /// Respond to a pending interaction. Returns true if the request was found.
    pub fn respond(&self, response: UserInteractionResponse) -> bool {
        if let Some(pending) = self.pending.lock().remove(&response.request_id) {
            let _ = pending.tx.send(response);
            true
        } else {
            false
        }
    }

    /// List all currently pending interaction requests.
    pub fn list_pending(&self) -> Vec<(String, InteractionKind)> {
        self.pending.lock().iter().map(|(id, p)| (id.clone(), p.kind.clone())).collect()
    }

    /// Inject a previously-persisted pending interaction into the gate
    /// so it is visible to `list_pending()` immediately.
    /// Used to reconstruct gate state after daemon restart.
    pub fn inject_pending(&self, request_id: String, kind: InteractionKind) {
        let (tx, _rx) = oneshot::channel();
        self.pending.lock().insert(request_id, PendingInteraction { tx, kind });
    }

    /// Close the gate by draining all pending interactions.
    /// Dropping the `oneshot::Sender`s causes any awaiting receivers to
    /// resolve with `RecvError`, unblocking agents stuck in `ask_user`
    /// or tool-approval waits.  Called before sending a Kill signal so
    /// the agent can process it promptly.
    pub fn close(&self) {
        self.pending.lock().clear();
    }

    /// Remove all pending interactions EXCEPT the one with the given
    /// request_id.  Returns the request IDs that were removed.
    /// Used to clean up stale injected entries when the agent creates
    /// a new question through the normal path.
    pub fn remove_all_except(&self, keep_request_id: &str) -> Vec<String> {
        let mut pending = self.pending.lock();
        let stale_ids: Vec<String> =
            pending.keys().filter(|id| id.as_str() != keep_request_id).cloned().collect();
        for id in &stale_ids {
            pending.remove(id);
        }
        stale_ids
    }
}

impl Drop for UserInteractionGate {
    fn drop(&mut self) {
        // Drain all pending interactions so their receivers resolve with
        // `RecvError` instead of hanging forever.  This prevents resource
        // leaks when a loop task is cancelled or the session shuts down.
        let pending = self.pending.get_mut();
        if !pending.is_empty() {
            tracing::debug!(
                count = pending.len(),
                "UserInteractionGate dropped with pending interactions"
            );
            pending.clear();
        }
    }
}

// ── Conversation Journal ────────────────────────────────────────────────────
// Strategy-agnostic log of model→tool cycles, persisted for mid-task resume.

/// A single tool call and its result, as recorded in the journal.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalToolCall {
    pub tool_id: String,
    pub input: String,
    pub output: String,
}

/// Identifies which phase of the loop strategy produced this entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JournalPhase {
    /// A ReAct iteration or PlanThenExecute inner tool loop.
    ToolCycle,
    /// PlanThenExecute: the plan was generated with these steps.
    Plan { steps: Vec<String> },
    /// PlanThenExecute: a step completed with its accumulated result text.
    StepComplete { step_index: usize, result: String },
}

/// One journal entry: a completed phase with its tool calls.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JournalEntry {
    pub phase: JournalPhase,
    pub turn: usize,
    pub tool_calls: Vec<JournalToolCall>,
}

/// Persistable log of all tool cycles executed during a loop run.
/// Used to reconstruct prompt state when resuming after a restart.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ConversationJournal {
    /// Which strategy produced this journal (e.g. "react", "plan_then_execute").
    pub strategy: Option<String>,
    pub entries: Vec<JournalEntry>,
}

/// Maximum number of journal entries before old ToolCycle entries are pruned.
/// Plan and StepComplete entries are always preserved.
const MAX_JOURNAL_ENTRIES: usize = 200;

impl ConversationJournal {
    /// Rebuild the full ReAct prompt from the initial task plus all tool cycles.
    pub fn reconstruct_react_prompt(&self, initial_prompt: &str) -> String {
        let mut prompt = initial_prompt.to_string();
        for entry in &self.entries {
            for tc in &entry.tool_calls {
                let safe_output = hive_contracts::prompt_sanitize::escape_prompt_tags(&tc.output);
                prompt.push_str(&format!(
                    "\n\n<tool_call>\n{{\"tool\": \"{}\", \"input\": {}}}\n</tool_call>\n<tool_result>\n{}\n</tool_result>",
                    tc.tool_id, tc.input, safe_output
                ));
            }
        }
        prompt
    }

    /// Number of completed tool iterations (for adaptive budget enforcement).
    pub fn tool_iteration_count(&self) -> usize {
        self.entries.iter().filter(|e| matches!(e.phase, JournalPhase::ToolCycle)).count()
    }

    /// Extract the plan steps if a Plan phase was journaled (PlanThenExecute).
    pub fn get_plan_steps(&self) -> Option<Vec<String>> {
        self.entries.iter().find_map(|e| match &e.phase {
            JournalPhase::Plan { steps } => Some(steps.clone()),
            _ => None,
        })
    }

    /// Get accumulated results from completed steps (PlanThenExecute).
    pub fn get_completed_step_results(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|e| match &e.phase {
                JournalPhase::StepComplete { result, .. } => Some(result.clone()),
                _ => None,
            })
            .collect()
    }

    /// Index of the last completed step (PlanThenExecute).
    pub fn last_completed_step_index(&self) -> Option<usize> {
        self.entries.iter().rev().find_map(|e| match &e.phase {
            JournalPhase::StepComplete { step_index, .. } => Some(*step_index),
            _ => None,
        })
    }

    /// Append a journal entry, pruning oldest ToolCycle entries if over the cap.
    pub fn record(&mut self, entry: JournalEntry) {
        self.entries.push(entry);
        // Prune oldest ToolCycle entries (preserve Plan/StepComplete for resume).
        while self.entries.len() > MAX_JOURNAL_ENTRIES {
            if let Some(pos) =
                self.entries.iter().position(|e| matches!(e.phase, JournalPhase::ToolCycle))
            {
                self.entries.remove(pos);
            } else {
                break;
            }
        }
    }
}

/// Build a human-readable summary of journal tool calls for a preempted turn.
/// Truncates individual tool outputs to keep the summary compact.
fn build_preemption_summary(journal: &ConversationJournal) -> String {
    let mut lines = Vec::new();
    lines.push("[Turn paused to process a new message]\n".to_string());
    lines.push("Progress so far:".to_string());

    let mut call_num = 0usize;
    for entry in &journal.entries {
        for tc in &entry.tool_calls {
            call_num += 1;
            let truncated_output = if tc.output.len() > 200 {
                format!("{}…", &tc.output[..200])
            } else {
                tc.output.clone()
            };
            lines.push(format!("{}. Called `{}` → {}", call_num, tc.tool_id, truncated_output));
        }
    }

    if call_num == 0 {
        lines.push("(no tool calls completed)".to_string());
    }

    lines.join("\n")
}

/// Check the preempt signal and, if set, build a preempted `LoopResult`.
/// Returns `Some(LoopResult)` when the loop should yield.
async fn check_preempt(
    signal: &Option<Arc<AtomicBool>>,
    journal: &Option<Arc<Mutex<ConversationJournal>>>,
    decision: &RoutingDecision,
    provider_id: &str,
    model: &str,
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
) -> Option<LoopResult> {
    let sig = signal.as_ref()?;
    if !sig.load(AtomicOrdering::Acquire) {
        return None;
    }

    let content = if let Some(ref j) = journal {
        build_preemption_summary(&j.lock())
    } else {
        "[Turn paused to process a new message]".to_string()
    };

    if let Some(tx) = event_tx {
        if tx.send(LoopEvent::Preempted).await.is_err() {
            tracing::warn!("failed to send Preempted event — loop receiver dropped");
        }
    }

    Some(LoopResult {
        content,
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        decision: decision.clone(),
        preempted: true,
    })
}

#[derive(Clone)]
pub struct ConversationContext {
    pub session_id: String,
    pub message_id: String,
    pub prompt: String,
    /// Multimodal content parts for the initial user prompt (text + images).
    /// Only populated on the first turn; subsequent tool-loop iterations use
    /// text-only prompts.
    pub prompt_content_parts: Vec<ContentPart>,
    pub history: Vec<CompletionMessage>,
    /// Shared journal for recording tool cycles (used for mid-task resume).
    pub conversation_journal: Option<Arc<Mutex<ConversationJournal>>>,
    /// Number of tool iterations already completed (from a prior journal on resume).
    pub initial_tool_iterations: usize,
}

#[derive(Clone)]
pub struct RoutingConfig {
    pub required_capabilities: BTreeSet<Capability>,
    pub preferred_models: Option<Vec<String>>,
    pub routing_decision: Option<RoutingDecision>,
    pub loop_strategy: Option<ConfigLoopStrategy>,
}

#[derive(Clone)]
pub struct SecurityContext {
    pub data_class: DataClass,
    /// Effective data-class, escalated as tools touch higher-class data.
    /// Initialized from `data_class`; only increases (never decreases).
    /// Shared `Arc<AtomicU8>` so it can be escalated through `&self`.
    pub effective_data_class: Arc<AtomicU8>,
    /// Per-session scoped permissions checked before tool definition approval.
    pub permissions: Arc<Mutex<SessionPermissions>>,
    pub workspace_classification: Option<Arc<WorkspaceClassification>>,
    /// Optional connector service handle for resolving output data-class
    /// when enforcing classification on outbound sends.
    pub connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
    /// When true, side-effecting external tool calls are intercepted and a
    /// synthetic success response is returned.  Built-in tools (`core.*`,
    /// `knowledge.*`) and read-only tools pass through unchanged.
    pub shadow_mode: bool,
}

#[derive(Clone)]
pub struct ToolsContext {
    pub tools: Arc<ToolRegistry>,
    /// How batched tool calls are executed (sequential-partial, sequential-full, parallel).
    pub tool_execution_mode: ToolExecutionMode,
    pub skill_catalog: Option<Arc<hive_skills::SkillCatalog>>,
    /// Handler for knowledge.query tool calls (provided by the API layer).
    pub knowledge_query_handler: Option<Arc<dyn KnowledgeQueryHandler>>,
}

#[derive(Clone)]
pub struct AgentContext {
    pub persona: Option<Persona>,
    pub personas: Vec<Persona>,
    pub current_agent_id: Option<String>,
    pub parent_agent_id: Option<String>,
    pub agent_orchestrator: Option<Arc<dyn AgentOrchestrator>>,
    /// The workspace directory for this agent. Child agents should inherit
    /// this so they operate in the same workspace as their parent.
    pub workspace_path: Option<PathBuf>,
    /// Whether this agent is one-shot (false) or service (true).
    pub keep_alive: bool,
    /// Set when a one-shot agent has already messaged the session.
    pub session_messaged: Arc<AtomicBool>,
}

pub struct LoopContext {
    pub conversation: ConversationContext,
    pub routing: RoutingConfig,
    pub security: SecurityContext,
    pub tools_ctx: ToolsContext,
    pub agent: AgentContext,
    /// Adaptive tool-call limits and stall detection config.
    /// Defaults to `ToolLimitsConfig::default()` if not explicitly set.
    pub tool_limits: ToolLimitsConfig,
    /// When set, the loop checks this signal after each tool batch.
    /// If `true`, the loop yields early so the next queued message
    /// can be processed at the current checkpoint.
    pub preempt_signal: Option<Arc<AtomicBool>>,
    /// When set, the loop can be cooperatively cancelled (e.g. on agent kill).
    /// Checked before model calls, between streaming chunks, and around tool
    /// execution so that in-flight operations are interrupted promptly.
    pub cancellation_token: Option<CancellationToken>,
}

impl LoopContext {
    /// Return the effective (possibly escalated) session data-class.
    pub fn effective_data_class(&self) -> DataClass {
        let raw = self.security.effective_data_class.load(AtomicOrdering::Acquire);
        DataClass::from_i64(raw as i64).unwrap_or(self.security.data_class)
    }

    /// Escalate the effective data-class if `new_class` is higher.
    pub fn escalate_data_class(&self, new_class: DataClass) {
        let new_val = new_class.to_i64() as u8;
        self.security.effective_data_class.fetch_max(new_val, AtomicOrdering::AcqRel);
    }

    // -- Accessor methods --
    pub fn session_id(&self) -> &str {
        &self.conversation.session_id
    }
    pub fn message_id(&self) -> &str {
        &self.conversation.message_id
    }
    pub fn prompt(&self) -> &str {
        &self.conversation.prompt
    }
    pub fn prompt_content_parts(&self) -> &[ContentPart] {
        &self.conversation.prompt_content_parts
    }
    pub fn history(&self) -> &[CompletionMessage] {
        &self.conversation.history
    }
    pub fn conversation_journal(&self) -> Option<&Arc<Mutex<ConversationJournal>>> {
        self.conversation.conversation_journal.as_ref()
    }
    pub fn initial_tool_iterations(&self) -> usize {
        self.conversation.initial_tool_iterations
    }
    pub fn required_capabilities(&self) -> &BTreeSet<Capability> {
        &self.routing.required_capabilities
    }
    pub fn preferred_models(&self) -> Option<&Vec<String>> {
        self.routing.preferred_models.as_ref()
    }
    pub fn routing_decision(&self) -> Option<&RoutingDecision> {
        self.routing.routing_decision.as_ref()
    }
    pub fn loop_strategy(&self) -> Option<&ConfigLoopStrategy> {
        self.routing.loop_strategy.as_ref()
    }
    pub fn data_class(&self) -> DataClass {
        self.security.data_class
    }
    pub fn permissions(&self) -> &Arc<Mutex<SessionPermissions>> {
        &self.security.permissions
    }
    pub fn workspace_classification(&self) -> Option<&Arc<WorkspaceClassification>> {
        self.security.workspace_classification.as_ref()
    }
    pub fn connector_service(&self) -> Option<&Arc<dyn hive_connectors::ConnectorServiceHandle>> {
        self.security.connector_service.as_ref()
    }
    pub fn tools(&self) -> &Arc<ToolRegistry> {
        &self.tools_ctx.tools
    }
    pub fn tool_execution_mode(&self) -> ToolExecutionMode {
        self.tools_ctx.tool_execution_mode
    }
    pub fn skill_catalog(&self) -> Option<&Arc<hive_skills::SkillCatalog>> {
        self.tools_ctx.skill_catalog.as_ref()
    }
    pub fn knowledge_query_handler(&self) -> Option<&Arc<dyn KnowledgeQueryHandler>> {
        self.tools_ctx.knowledge_query_handler.as_ref()
    }
    pub fn persona(&self) -> Option<&Persona> {
        self.agent.persona.as_ref()
    }
    pub fn personas(&self) -> &[Persona] {
        &self.agent.personas
    }
    pub fn current_agent_id(&self) -> Option<&str> {
        self.agent.current_agent_id.as_deref()
    }
    pub fn parent_agent_id(&self) -> Option<&str> {
        self.agent.parent_agent_id.as_deref()
    }
    pub fn agent_orchestrator(&self) -> Option<&Arc<dyn AgentOrchestrator>> {
        self.agent.agent_orchestrator.as_ref()
    }
    pub fn keep_alive(&self) -> bool {
        self.agent.keep_alive
    }
    pub fn session_messaged(&self) -> &Arc<AtomicBool> {
        &self.agent.session_messaged
    }
    pub fn workspace_path(&self) -> Option<&Path> {
        self.agent.workspace_path.as_deref()
    }
}

#[allow(clippy::too_many_arguments)]
pub trait AgentOrchestrator: Send + Sync {
    fn spawn_agent(
        &self,
        persona: Persona,
        task: String,
        from: Option<String>,
        friendly_name: Option<String>,
        data_class: hive_classification::DataClass,
        parent_model: Option<hive_model::ModelSelection>,
        keep_alive: bool,
        workspace_path: Option<PathBuf>,
    ) -> BoxFuture<'_, Result<String, String>>;

    fn message_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    /// Send a message from an agent back to the parent chat session.
    fn message_session(
        &self,
        message: String,
        from_agent_id: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    /// Send a feedback (non-executing) message to an agent. Unlike `message_agent`,
    /// this does NOT trigger a new task execution — the agent merely logs the content.
    fn feedback_agent(
        &self,
        agent_id: String,
        message: String,
        from: String,
    ) -> BoxFuture<'_, Result<(), String>>;

    #[allow(clippy::type_complexity)]
    fn list_agents(
        &self,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>>;

    /// Retrieve the final result of a completed agent by ID.
    fn get_agent_result(
        &self,
        agent_id: String,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>>;

    fn kill_agent(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>>;

    /// Block until the given agent reaches a terminal state (done/error) or timeout.
    /// Returns `(status, result)`.
    fn wait_for_agent(
        &self,
        agent_id: String,
        timeout_secs: Option<u64>,
    ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
        let _ = (agent_id, timeout_secs);
        Box::pin(async { Err("wait_for_agent is not supported in this context".to_string()) })
    }

    /// Search bots by keyword. Returns (id, name, description) tuples.
    /// Default: no bots available.
    #[allow(clippy::type_complexity)]
    fn search_bots(
        &self,
        _query: String,
    ) -> BoxFuture<'_, Result<Vec<(String, String, String)>, String>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Get the parent agent ID for a given agent.
    /// Returns `Ok(None)` if the agent exists but has no parent (root-level).
    /// Returns `Ok(Some(parent_id))` if the agent has a parent.
    /// Returns `Err` if the agent is not found or not supported.
    fn get_agent_parent(&self, _agent_id: String) -> BoxFuture<'_, Result<Option<String>, String>> {
        Box::pin(async { Err("get_agent_parent is not supported in this context".to_string()) })
    }
}

/// Handler for `knowledge.query` tool calls.
///
/// Implemented by the API layer which has access to the knowledge graph.
/// The loop layer intercepts `knowledge.query` calls and delegates to this
/// trait, similar to `AgentOrchestrator`.
pub trait KnowledgeQueryHandler: Send + Sync {
    /// Execute a knowledge graph query and return the JSON result.
    fn handle_query(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, String>>;
}

#[derive(Debug, Clone)]
pub struct LoopResult {
    pub content: String,
    pub provider_id: String,
    pub model: String,
    pub decision: RoutingDecision,
    /// `true` when the loop yielded early because a new user message
    /// was enqueued (the preempt signal fired). The `content` field
    /// contains a summary of tool work completed so far.
    pub preempted: bool,
}

/// Streaming filter that suppresses `<tool_call>`, `<function_call>`,
/// `<tool_use>`, and `<tool_result>` XML blocks from streamed token
/// deltas so they are not shown to the user.
///
/// Accumulates text in a buffer while a potential tag is being formed.
/// Once a complete opening tag is recognised, all content until the
/// matching close tag is swallowed.  If the buffer turns out not to
/// match any known tag, it is flushed as normal output.
struct StreamingToolCallFilter {
    /// Buffered text that might be the start of a tool tag.
    buffer: String,
    /// When `true`, we are inside a recognised tool-call block and
    /// suppressing all content until the matching close tag.
    suppressing: bool,
    /// The close tag we are looking for (e.g. `</tool_call>`).
    close_tag: &'static str,
}

impl StreamingToolCallFilter {
    const OPEN_TAGS: [(&'static str, &'static str); 4] = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
        ("<tool_result>", "</tool_result>"),
    ];

    fn new() -> Self {
        Self { buffer: String::new(), suppressing: false, close_tag: "" }
    }

    /// Feed a streaming delta and return text that should be emitted
    /// to the user.  Returns an empty string when the content is being
    /// suppressed (inside a tool block) or buffered (potential tag start).
    fn feed(&mut self, delta: &str) -> String {
        if self.suppressing {
            self.buffer.push_str(delta);
            if let Some(pos) = self.buffer.find(self.close_tag) {
                // End of suppressed block — return any text after the close tag
                let after = self.buffer[pos + self.close_tag.len()..].to_string();
                self.buffer.clear();
                self.suppressing = false;
                if after.is_empty() {
                    return String::new();
                }
                // Recursively feed the remainder in case there are more tags
                return self.feed(&after);
            }
            // Still inside the block — suppress everything
            return String::new();
        }

        self.buffer.push_str(delta);

        // Check if the buffer contains (or could start) an opening tag.
        // We need to handle the case where the tag arrives across
        // multiple deltas (e.g. "<", "tool", "_call>").
        for &(open, close) in &Self::OPEN_TAGS {
            if let Some(tag_start) = self.buffer.find(open) {
                // Full opening tag found — start suppressing
                let before = self.buffer[..tag_start].to_string();
                let rest = self.buffer[tag_start + open.len()..].to_string();
                self.buffer = rest;
                self.suppressing = true;
                self.close_tag = close;
                // Check if the close tag is already in the buffer
                let suppressed = self.feed("");
                let mut out = before;
                out.push_str(&suppressed);
                return out;
            }
            // Check if the buffer ends with a prefix of an opening tag
            if could_be_tag_prefix(&self.buffer, open) {
                // Hold the buffer — don't emit yet
                return String::new();
            }
        }

        // No tag match — flush the buffer

        std::mem::take(&mut self.buffer)
    }

    /// Flush any remaining buffered text (call at end of stream).
    fn flush(&mut self) -> String {
        std::mem::take(&mut self.buffer)
    }
}

/// Check if `buffer` ends with a non-empty prefix of `tag`.
fn could_be_tag_prefix(buffer: &str, tag: &str) -> bool {
    // Check if any suffix of buffer matches a prefix of tag
    let buf_bytes = buffer.as_bytes();
    let tag_bytes = tag.as_bytes();
    for start in (0..buf_bytes.len()).rev() {
        let suffix = &buf_bytes[start..];
        if suffix.len() >= tag_bytes.len() {
            break; // suffix is longer than the tag — already checked for full match
        }
        if tag_bytes.starts_with(suffix) {
            return true;
        }
    }
    false
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LoopEvent {
    /// The model is being loaded into memory (local models only)
    ModelLoading {
        provider_id: String,
        model: String,
        /// Count of tool results by tool name included in this model call.
        tool_result_counts: HashMap<String, u32>,
        /// Estimated token count for the outgoing request (prompt + history + tools).
        estimated_tokens: Option<u32>,
    },
    /// A token chunk from the model
    Token { delta: String },
    /// Model finished generating (may include tool call)
    ModelDone { content: String, provider_id: String, model: String },
    /// A tool call is starting
    ToolCallStart { tool_id: String, input: String },
    /// A tool call completed
    ToolCallResult { tool_id: String, output: String, is_error: bool },
    /// User interaction required (tool approval, question, etc.)
    UserInteractionRequired { request_id: String, kind: InteractionKind },
    /// The loop is complete with final result
    Done { content: String, provider_id: String, model: String },
    /// An error occurred
    Error {
        message: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        http_status: Option<u16>,
        /// Provider that produced the error.
        provider_id: Option<String>,
        /// Model that produced the error.
        model: Option<String>,
    },
    /// A transient LLM error triggered a retry with backoff.
    ModelRetry {
        provider_id: String,
        model: String,
        attempt: u32,
        max_attempts: u32,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_kind: String,
        http_status: Option<u16>,
        backoff_ms: u64,
    },
    /// A message was injected into the session by an agent
    AgentSessionMessage { from_agent_id: String, content: String },
    /// The selected model was unavailable; fell back to an alternative.
    ModelFallback {
        from_provider: String,
        from_model: String,
        to_provider: String,
        to_model: String,
    },
    /// The tool-call budget was extended because the agent is making progress.
    BudgetExtended { new_budget: usize, extensions_granted: usize },
    /// The stall detector noticed repeated identical tool calls (warning before stop).
    StallWarning { tool_name: String, repeated_count: usize },
    /// The loop is yielding early because a new user message was enqueued.
    Preempted,
    /// A side-effecting tool call was intercepted in shadow mode.
    ToolCallIntercepted { tool_id: String, input: String },
    /// Partial tool-call argument snapshot during streaming.
    ToolCallArgDelta {
        index: usize,
        call_id: Option<String>,
        tool_name: Option<String>,
        arguments_so_far: String,
    },
}

#[derive(Debug, Error)]
pub enum LoopError {
    #[error("model routing failed: {0}")]
    ModelRouting(String),
    #[error("model execution failed: {message}")]
    ModelExecution {
        message: String,
        /// Classified error kind (e.g. "rate_limited", "server_error").
        error_code: Option<String>,
        /// HTTP status code from the provider, if available.
        http_status: Option<u16>,
        /// Provider that produced the error.
        provider_id: Option<String>,
        /// Model that produced the error.
        model: Option<String>,
    },
    #[error("model worker join failed: {0}")]
    JoinFailed(String),
    #[error("middleware rejected request: {0}")]
    MiddlewareRejected(String),
    #[error("tool `{tool_id}` is not registered")]
    ToolUnavailable { tool_id: String },
    #[error("tool `{tool_id}` is denied by policy: {reason}")]
    ToolDenied { tool_id: String, reason: String },
    #[error("tool `{tool_id}` requires approval")]
    ToolApprovalRequired { tool_id: String },
    #[error("tool `{tool_id}` failed: {detail}")]
    ToolExecutionFailed { tool_id: String, detail: String },
    #[error("tool call limit reached ({limit})")]
    ToolCallLimit { limit: usize },
    #[error("stall detected: tool `{tool_name}` called {count} times with identical arguments")]
    StallDetected { tool_name: String, count: usize },
    #[error("hard tool call ceiling reached ({ceiling})")]
    HardCeilingReached { ceiling: usize },
    #[error("operation cancelled")]
    Cancelled,
}

/// Tools that are exempt from the adaptive tool-call budget.
/// These are lightweight status/polling tools that don't represent
/// forward progress and shouldn't consume the agent's budget.
fn is_budget_exempt(tool_id: &str) -> bool {
    matches!(
        tool_id,
        "core.list_agents"
            | "core.get_agent_result"
            | "core.wait_for_agent"
            | "process.status"
            | "process.list"
    )
}

/// Convert a [`ModelRouterError`] into a [`LoopError::ModelExecution`] with
/// structured error fields extracted from the router error.
fn model_router_error_to_loop_error(error: ModelRouterError) -> LoopError {
    match &error {
        ModelRouterError::ProviderExecutionFailed { error_kind, http_status, .. } => {
            LoopError::ModelExecution {
                error_code: error_kind.map(|k| format!("{k:?}").to_lowercase()),
                http_status: *http_status,
                provider_id: None,
                model: None,
                message: error.to_string(),
            }
        }
        _ => LoopError::ModelExecution {
            message: error.to_string(),
            error_code: None,
            http_status: None,
            provider_id: None,
            model: None,
        },
    }
}

/// Build a simple [`LoopError::ModelExecution`] from a string (for non-router
/// errors like mid-stream failures or join errors).
pub(crate) fn simple_model_error(message: String) -> LoopError {
    LoopError::ModelExecution {
        message,
        error_code: None,
        http_status: None,
        provider_id: None,
        model: None,
    }
}

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
}

pub trait LoopStrategy: Send + Sync {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
    ) -> BoxFuture<'a, Result<LoopResult, LoopError>>;
}

#[derive(Default)]
pub struct ReActStrategy;

impl LoopStrategy for ReActStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
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

            let mut prompt = context.conversation.prompt.clone();
            let mut tool_iterations = context.conversation.initial_tool_iterations;
            // Include multimodal content parts only on the very first LLM call.
            let mut prompt_content_parts = context.conversation.prompt_content_parts.clone();
            // Cumulative count of tool results by tool name across iterations.
            let mut tool_result_counts: HashMap<String, u32> = HashMap::new();

            // Adaptive tool-call budget
            let mut budget = crate::tool_budget::AdaptiveBudget::new(&context.tool_limits);

            loop {
                let mut request = CompletionRequest {
                    prompt: prompt.clone(),
                    prompt_content_parts: std::mem::take(&mut prompt_content_parts),
                    messages: context.conversation.history.clone(),
                    required_capabilities: context.routing.required_capabilities.clone(),
                    preferred_models: context.routing.preferred_models.clone(),
                    tools: context.tools_ctx.tools.list_definitions(),
                };

                // Log tool count and any MCP tools reaching the LLM.
                let mcp_tools: Vec<&str> = request
                    .tools
                    .iter()
                    .filter(|t| t.id.starts_with("mcp."))
                    .map(|t| t.id.as_str())
                    .collect();
                if !mcp_tools.is_empty() {
                    tracing::info!(
                        total = request.tools.len(),
                        mcp_count = mcp_tools.len(),
                        mcp_tools = ?mcp_tools,
                        "tools in CompletionRequest"
                    );
                }

                for hook in middleware {
                    request = hook.before_model_call(&context, request)?;
                }

                let response = if let Some(ref tx) = event_tx {
                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();

                    // Signal that we're about to call the model (may trigger
                    // loading a local model into memory — a slow operation).
                    let _ = tx.try_send(LoopEvent::ModelLoading {
                        provider_id: decision_clone.selected.provider_id.clone(),
                        model: decision_clone.selected.model.clone(),
                        tool_result_counts: tool_result_counts.clone(),
                        estimated_tokens: Some(estimate_request_tokens(&request)),
                    });

                    let retry_cb = |info: &RetryInfo| {
                        let _ = tx.try_send(LoopEvent::ModelRetry {
                            provider_id: info.provider_id.clone(),
                            model: info.model.clone(),
                            attempt: info.attempt,
                            max_attempts: info.max_attempts,
                            error_kind: format!("{:?}", info.error_kind).to_lowercase(),
                            http_status: info.http_status,
                            backoff_ms: info.backoff_ms,
                        });
                    };

                    let (stream, actual_selection) = router
                        .complete_stream_with_decision_and_callback(
                            &request,
                            &decision_clone,
                            Some(&retry_cb),
                        )
                        .map_err(model_router_error_to_loop_error)?;

                    // Emit fallback notification if the model differs from the originally selected one.
                    if actual_selection != decision_clone.selected
                        && tx
                            .try_send(LoopEvent::ModelFallback {
                                from_provider: decision_clone.selected.provider_id.clone(),
                                from_model: decision_clone.selected.model.clone(),
                                to_provider: actual_selection.provider_id.clone(),
                                to_model: actual_selection.model.clone(),
                            })
                            .is_err()
                    {
                        tracing::warn!(
                            "failed to send ModelFallback event — channel full or closed"
                        );
                    }

                    let mut content = String::new();
                    let provider_id = actual_selection.provider_id.clone();
                    let model = actual_selection.model.clone();
                    let mut streamed_tool_calls = Vec::new();
                    let mut token_filter = StreamingToolCallFilter::new();

                    tokio::pin!(stream);
                    let stream_cancelled;
                    loop {
                        let chunk_result = if let Some(ref token) = context.cancellation_token {
                            tokio::select! {
                                biased;
                                _ = token.cancelled() => {
                                    stream_cancelled = true;
                                    break;
                                }
                                chunk = stream.next() => chunk,
                            }
                        } else {
                            stream.next().await
                        };
                        match chunk_result {
                            Some(Ok(chunk)) => {
                                if !chunk.delta.is_empty() {
                                    // Always accumulate full content for parsing.
                                    content.push_str(&chunk.delta);
                                    // Filter out <tool_call> blocks before
                                    // sending tokens to the UI.
                                    let visible = token_filter.feed(&chunk.delta);
                                    if !visible.is_empty() {
                                        let _ = tx.try_send(LoopEvent::Token { delta: visible });
                                    }
                                }
                                // Emit partial tool-call argument snapshots
                                // only for MCP server tools (id pattern: mcp.{server}.{tool},
                                // sanitized to mcp_{server}_{tool}).  Skipping internal
                                // tools like core_ask_user avoids flooding the event log.
                                for d in &chunk.tool_call_arg_deltas {
                                    let is_mcp_tool = d.name.as_deref()
                                        .map(|n| n.starts_with("mcp_") || n.starts_with("mcp.") || n.starts_with("app."))
                                        .unwrap_or(false);
                                    if is_mcp_tool {
                                        let _ = tx.try_send(LoopEvent::ToolCallArgDelta {
                                            index: d.index,
                                            call_id: d.call_id.clone(),
                                            tool_name: d.name.clone(),
                                            arguments_so_far: d.arguments_so_far.clone(),
                                        });
                                    }
                                }
                                if !chunk.tool_calls.is_empty() {
                                    streamed_tool_calls.extend(chunk.tool_calls);
                                }
                            }
                            Some(Err(e)) => return Err(simple_model_error(format!("{:#}", e))),
                            None => {
                                stream_cancelled = false;
                                break;
                            }
                        }
                    }
                    if stream_cancelled {
                        return Err(LoopError::Cancelled);
                    }
                    // Flush any remaining buffered text that wasn't part of a tag
                    let remaining = token_filter.flush();
                    if !remaining.is_empty() {
                        let _ = tx.try_send(LoopEvent::Token { delta: remaining });
                    }

                    let _ = tx.try_send(LoopEvent::ModelDone {
                        content: content.clone(),
                        provider_id: provider_id.clone(),
                        model: model.clone(),
                    });

                    CompletionResponse {
                        provider_id,
                        model,
                        content,
                        tool_calls: streamed_tool_calls,
                    }
                } else {
                    let router = Arc::clone(&model_router);
                    let decision_clone = decision.clone();
                    let request_clone = request.clone();
                    let blocking_future = tokio::task::spawn_blocking(move || {
                        router.complete_with_decision(&request_clone, &decision_clone)
                    });
                    if let Some(ref token) = context.cancellation_token {
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => {
                                return Err(LoopError::Cancelled);
                            }
                            result = blocking_future => {
                                result
                                    .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                                    .map_err(model_router_error_to_loop_error)?
                            }
                        }
                    } else {
                        blocking_future
                            .await
                            .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                            .map_err(model_router_error_to_loop_error)?
                    }
                };

                let mut response = response;
                for hook in middleware {
                    response = hook.after_model_response(&context, response)?;
                }

                // Prefer native structured tool calls from the provider
                let detected_calls: Vec<ToolCall> = if !response.tool_calls.is_empty() {
                    response
                        .tool_calls
                        .iter()
                        .map(|tc| ToolCall {
                            tool_id: tc.name.clone(),
                            input: tc.arguments.clone(),
                        })
                        .collect()
                } else {
                    // Fallback: text-based extraction for providers without native tool calls
                    parse_tool_calls(&response.content)
                };

                if !detected_calls.is_empty() {
                    let billable_count =
                        detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                    // Check adaptive budget BEFORE executing the batch.
                    match budget.check(tool_iterations, billable_count) {
                        crate::tool_budget::BudgetDecision::Allow => { /* proceed */ }
                        crate::tool_budget::BudgetDecision::Extended {
                            new_budget,
                            extensions_granted,
                        } => {
                            if let Some(ref tx) = event_tx {
                                let _ = tx.try_send(LoopEvent::BudgetExtended {
                                    new_budget,
                                    extensions_granted,
                                });
                            }
                            tracing::info!(
                                new_budget,
                                extensions_granted,
                                "tool-call budget extended — agent is making progress"
                            );
                        }
                        crate::tool_budget::BudgetDecision::HardStop { ceiling } => {
                            return Err(LoopError::HardCeilingReached { ceiling });
                        }
                    }

                    let (tool_results, journal_tool_calls) = execute_tool_batch(
                        &detected_calls,
                        &context,
                        middleware,
                        event_tx.as_ref(),
                        interaction_gate.as_deref(),
                        Some(&response.content),
                    )
                    .await;

                    for jtc in &journal_tool_calls {
                        *tool_result_counts.entry(jtc.tool_id.clone()).or_insert(0) += 1;
                    }

                    prompt = format!("{prompt}{tool_results}");
                    // Count individual tool calls, not just iterations, so the
                    // limit reflects actual work done. Exempt polling/status tools.
                    tool_iterations +=
                        detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();

                    if let Some(ref journal) = context.conversation.conversation_journal {
                        let mut j = journal.lock();
                        j.record(JournalEntry {
                            phase: JournalPhase::ToolCycle,
                            turn: tool_iterations,
                            tool_calls: journal_tool_calls,
                        });
                    }

                    // Check if a new user message is waiting — yield at this checkpoint.
                    if let Some(result) = check_preempt(
                        &context.preempt_signal,
                        &context.conversation.conversation_journal,
                        &decision,
                        &response.provider_id,
                        &response.model,
                        event_tx.as_ref(),
                    )
                    .await
                    {
                        return Ok(result);
                    }

                    continue;
                }

                if let Some(ref tx) = event_tx {
                    // Done is a critical event — use blocking send to avoid silent loss.
                    let _ = tx
                        .send(LoopEvent::Done {
                            content: response.content.clone(),
                            provider_id: response.provider_id.clone(),
                            model: response.model.clone(),
                        })
                        .await;
                }

                return Ok(LoopResult {
                    content: response.content,
                    provider_id: response.provider_id,
                    model: response.model,
                    decision,
                    preempted: false,
                });
            }
        })
    }
}

#[derive(Default)]
pub struct SequentialStrategy;

impl LoopStrategy for SequentialStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
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
            let response = if let Some(ref token) = context.cancellation_token {
                tokio::select! {
                    biased;
                    _ = token.cancelled() => {
                        return Err(LoopError::Cancelled);
                    }
                    result = blocking_future => {
                        result
                            .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                            .map_err(model_router_error_to_loop_error)?
                    }
                }
            } else {
                blocking_future
                    .await
                    .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                    .map_err(model_router_error_to_loop_error)?
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

#[derive(Default)]
pub struct PlanThenExecuteStrategy;

impl PlanThenExecuteStrategy {
    fn parse_plan(content: &str) -> Vec<String> {
        content
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                // Match lines like "1. ...", "2) ...", "- ..." etc.
                if let Some(rest) = trimmed.strip_prefix("- ") {
                    Some(rest.trim().to_string())
                } else if let Some(pos) = trimmed.find(". ") {
                    let prefix = &trimmed[..pos];
                    if prefix.chars().all(|c| c.is_ascii_digit()) {
                        Some(trimmed[pos + 2..].trim().to_string())
                    } else {
                        None
                    }
                } else if let Some(pos) = trimmed.find(") ") {
                    let prefix = &trimmed[..pos];
                    if prefix.chars().all(|c| c.is_ascii_digit()) {
                        Some(trimmed[pos + 2..].trim().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect()
    }
}

impl LoopStrategy for PlanThenExecuteStrategy {
    fn run<'a>(
        &'a self,
        context: LoopContext,
        model_router: Arc<ModelRouter>,
        middleware: &'a [Arc<dyn LoopMiddleware>],
        event_tx: Option<tokio::sync::mpsc::Sender<LoopEvent>>,
        interaction_gate: Option<Arc<UserInteractionGate>>,
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

            // Check if we're resuming from a journal
            let (steps, mut accumulated_results, resume_step) = {
                let journal_ref = context.conversation.conversation_journal.as_ref();
                let journal_guard = journal_ref.map(|j| j.lock());

                if let Some(j) = journal_guard.filter(|j| !j.entries.is_empty()) {
                    if let Some(plan_steps) = j.get_plan_steps() {
                        let completed_results = j.get_completed_step_results();
                        let last_step = j.last_completed_step_index().map(|i| i + 1).unwrap_or(0);
                        (Some(plan_steps), completed_results, last_step)
                    } else {
                        (None, Vec::new(), 0)
                    }
                } else {
                    (None, Vec::new(), 0)
                }
            };

            let steps = if let Some(steps) = steps {
                // Resuming with a previously generated plan
                steps
            } else {
                // Phase 1: Ask the model for a plan
                let plan_prompt = format!(
                    "Create a numbered plan (one step per line) to accomplish the following task. \
                     Output ONLY the numbered steps, nothing else.\n\nTask: {}",
                    context.conversation.prompt
                );

                let mut plan_request = CompletionRequest {
                    prompt: plan_prompt,
                    prompt_content_parts: context.conversation.prompt_content_parts.clone(),
                    messages: context.conversation.history.clone(),
                    required_capabilities: context.routing.required_capabilities.clone(),
                    preferred_models: context.routing.preferred_models.clone(),
                    tools: context.tools_ctx.tools.list_definitions(),
                };

                for hook in middleware {
                    plan_request = hook.before_model_call(&context, plan_request)?;
                }

                let router = Arc::clone(&model_router);
                let decision_clone = decision.clone();
                let request_clone = plan_request.clone();
                let blocking_future = tokio::task::spawn_blocking(move || {
                    router.complete_with_decision(&request_clone, &decision_clone)
                });
                let plan_response = if let Some(ref token) = context.cancellation_token {
                    tokio::select! {
                        biased;
                        _ = token.cancelled() => {
                            return Err(LoopError::Cancelled);
                        }
                        result = blocking_future => {
                            result
                                .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                                .map_err(model_router_error_to_loop_error)?
                        }
                    }
                } else {
                    blocking_future
                        .await
                        .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                        .map_err(model_router_error_to_loop_error)?
                };

                let mut plan_response = plan_response;
                for hook in middleware {
                    plan_response = hook.after_model_response(&context, plan_response)?;
                }

                let mut parsed_steps = Self::parse_plan(&plan_response.content);
                parsed_steps.truncate(MAX_PLAN_STEPS);

                if parsed_steps.is_empty() {
                    return Ok(LoopResult {
                        content: plan_response.content,
                        provider_id: plan_response.provider_id,
                        model: plan_response.model,
                        decision,
                        preempted: false,
                    });
                }

                // Journal the plan
                if let Some(ref journal) = context.conversation.conversation_journal {
                    let mut j = journal.lock();
                    j.record(JournalEntry {
                        phase: JournalPhase::Plan { steps: parsed_steps.clone() },
                        turn: 0,
                        tool_calls: Vec::new(),
                    });
                }

                parsed_steps
            };

            // Phase 2: Execute each step with adaptive tool-call limits
            let mut last_response = CompletionResponse {
                provider_id: String::new(),
                model: String::new(),
                content: String::new(),
                tool_calls: Vec::new(),
            };

            // Shared adaptive budget across all plan steps.
            let mut budget = crate::tool_budget::AdaptiveBudget::new(&context.tool_limits);
            let mut total_tool_calls = 0usize;

            for (step_idx, step) in steps.iter().enumerate() {
                // Skip already-completed steps when resuming
                if step_idx < resume_step {
                    continue;
                }

                let mut step_prompt = format!(
                    "You are executing a plan step by step.\n\n\
                     Original task: {}\n\n\
                     Completed so far:\n{}\n\n\
                     Current step: {}",
                    context.conversation.prompt,
                    accumulated_results.join("\n"),
                    step
                );

                let mut _tool_calls_in_step = 0usize;

                loop {
                    let mut request = CompletionRequest {
                        prompt: step_prompt.clone(),
                        prompt_content_parts: context.conversation.prompt_content_parts.clone(),
                        messages: context.conversation.history.clone(),
                        required_capabilities: context.routing.required_capabilities.clone(),
                        preferred_models: context.routing.preferred_models.clone(),
                        tools: context.tools_ctx.tools.list_definitions(),
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
                    let response = if let Some(ref token) = context.cancellation_token {
                        tokio::select! {
                            biased;
                            _ = token.cancelled() => {
                                return Err(LoopError::Cancelled);
                            }
                            result = blocking_future => {
                                result
                                    .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                                    .map_err(model_router_error_to_loop_error)?
                            }
                        }
                    } else {
                        blocking_future
                            .await
                            .map_err(|error| LoopError::JoinFailed(error.to_string()))?
                            .map_err(model_router_error_to_loop_error)?
                    };

                    let mut response = response;
                    for hook in middleware {
                        response = hook.after_model_response(&context, response)?;
                    }

                    // Prefer native structured tool calls from the provider
                    let detected_calls: Vec<ToolCall> = if !response.tool_calls.is_empty() {
                        response
                            .tool_calls
                            .iter()
                            .map(|tc| ToolCall {
                                tool_id: tc.name.clone(),
                                input: tc.arguments.clone(),
                            })
                            .collect()
                    } else {
                        parse_tool_calls(&response.content)
                    };

                    if !detected_calls.is_empty() {
                        let billable_count =
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                        // Check adaptive budget BEFORE executing the batch.
                        match budget.check(total_tool_calls, billable_count) {
                            crate::tool_budget::BudgetDecision::Allow => { /* proceed */ }
                            crate::tool_budget::BudgetDecision::Extended {
                                new_budget,
                                extensions_granted,
                            } => {
                                if let Some(ref tx) = event_tx {
                                    let _ = tx.try_send(LoopEvent::BudgetExtended {
                                        new_budget,
                                        extensions_granted,
                                    });
                                }
                                tracing::info!(
                                    new_budget,
                                    extensions_granted,
                                    step = step_idx,
                                    "tool-call budget extended in plan step"
                                );
                            }
                            crate::tool_budget::BudgetDecision::HardStop { ceiling } => {
                                return Err(LoopError::HardCeilingReached { ceiling });
                            }
                        }

                        let (tool_results, journal_tool_calls) = execute_tool_batch(
                            &detected_calls,
                            &context,
                            middleware,
                            event_tx.as_ref(),
                            interaction_gate.as_deref(),
                            Some(&response.content),
                        )
                        .await;

                        if tool_results.is_empty() {
                            return Err(simple_model_error(
                                "tool calls returned no results".to_string(),
                            ));
                        }

                        step_prompt = format!("{step_prompt}{tool_results}");
                        let billable =
                            detected_calls.iter().filter(|c| !is_budget_exempt(&c.tool_id)).count();
                        _tool_calls_in_step += billable;
                        total_tool_calls += billable;

                        if let Some(ref journal) = context.conversation.conversation_journal {
                            let mut j = journal.lock();
                            j.record(JournalEntry {
                                phase: JournalPhase::ToolCycle,
                                turn: step_idx + 1,
                                tool_calls: journal_tool_calls,
                            });
                        }

                        // Check if a new user message is waiting — yield at this checkpoint.
                        if let Some(result) = check_preempt(
                            &context.preempt_signal,
                            &context.conversation.conversation_journal,
                            &decision,
                            &response.provider_id,
                            &response.model,
                            event_tx.as_ref(),
                        )
                        .await
                        {
                            return Ok(result);
                        }

                        continue;
                    }

                    let step_result = format!("- {}: {}", step, response.content);
                    accumulated_results.push(step_result.clone());
                    last_response = response;

                    // Journal step completion
                    if let Some(ref journal) = context.conversation.conversation_journal {
                        let mut j = journal.lock();
                        j.record(JournalEntry {
                            phase: JournalPhase::StepComplete {
                                step_index: step_idx,
                                result: step_result,
                            },
                            turn: step_idx + 1,
                            tool_calls: Vec::new(),
                        });
                    }

                    break;
                }
            }

            Ok(LoopResult {
                content: last_response.content,
                provider_id: last_response.provider_id,
                model: last_response.model,
                decision,
                preempted: false,
            })
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyKind {
    ReAct,
    Sequential,
    PlanThenExecute,
}

impl StrategyKind {
    pub fn build(&self) -> Arc<dyn LoopStrategy> {
        match self {
            StrategyKind::ReAct => Arc::new(ReActStrategy),
            StrategyKind::Sequential => Arc::new(SequentialStrategy),
            StrategyKind::PlanThenExecute => Arc::new(PlanThenExecuteStrategy),
        }
    }
}

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
            Some(ConfigLoopStrategy::React) => Arc::new(ReActStrategy),
            Some(ConfigLoopStrategy::Sequential) => Arc::new(SequentialStrategy),
            Some(ConfigLoopStrategy::PlanThenExecute) => Arc::new(PlanThenExecuteStrategy),
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
        event_tx: tokio::sync::mpsc::Sender<LoopEvent>,
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
        let result = execute_tool_call(
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

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub tool_id: String,
    pub input: Value,
}

const MAX_PLAN_STEPS: usize = 10;

/// Maximum characters for a single tool output before it is truncated in the prompt.
/// ~25K tokens — large enough for most file reads but prevents a single tool from
/// consuming the entire context window.
const MAX_TOOL_OUTPUT_CHARS: usize = 100_000;

/// Return the largest prefix of `s` whose byte length is ≤ `max_bytes`,
/// without splitting a multi-byte UTF-8 character.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Truncate a tool output string if it exceeds `MAX_TOOL_OUTPUT_CHARS`.
fn cap_tool_output(output: &str) -> String {
    if output.len() <= MAX_TOOL_OUTPUT_CHARS {
        output.to_string()
    } else {
        let keep = MAX_TOOL_OUTPUT_CHARS.saturating_sub(80);
        let truncated = truncate_str(output, keep);
        format!(
            "{}…\n\n[output truncated — {total} chars total, showing first {shown}]",
            truncated,
            total = output.len(),
            shown = truncated.len(),
        )
    }
}

/// Estimate the number of tokens in a completion request.
///
/// Uses the common heuristic of ~4 characters per token (English text).
/// Accounts for prompt, conversation history, and tool definitions.
fn estimate_request_tokens(request: &CompletionRequest) -> u32 {
    let mut chars: usize = request.prompt.len();
    for msg in &request.messages {
        // ~4 tokens overhead per message for role/separators
        chars += msg.role.len() + msg.content.len() + 16;
    }
    for tool in &request.tools {
        chars += tool.id.len() + tool.name.len() + tool.description.len();
        chars += tool.input_schema.to_string().len();
    }
    (chars / 4) as u32
}

/// Result of a single tool call execution (success or error).
struct ToolCallOutcome {
    tool_id: String,
    input_str: String,
    output: String,
    is_error: bool,
    /// The tool's channel classification, used for post-execution
    /// re-verification in parallel batches (TOCTOU mitigation).
    channel_class: Option<ChannelClass>,
}

/// Execute a batch of tool calls according to the configured
/// [`ToolExecutionMode`].
async fn execute_tool_batch(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> (String, Vec<JournalToolCall>) {
    // Strip tool-call XML blocks from assistant_content so that handlers
    // (e.g. core.ask_user) don't leak raw <tool_call> tags into
    // user-visible output such as the question `message` field.
    let cleaned_content = assistant_content.map(strip_xml_tool_blocks);
    let cleaned_ref = cleaned_content.as_deref();

    let mode = context.tools_ctx.tool_execution_mode;
    let outcomes = match mode {
        ToolExecutionMode::Parallel => {
            execute_tools_parallel(
                calls,
                context,
                middleware,
                event_tx,
                interaction_gate,
                cleaned_ref,
            )
            .await
        }
        _ => {
            let stop_on_error = mode == ToolExecutionMode::SequentialPartial;
            execute_tools_sequential(
                calls,
                context,
                middleware,
                event_tx,
                interaction_gate,
                stop_on_error,
                cleaned_ref,
            )
            .await
        }
    };

    // Post-execution re-verification for parallel mode: if the session's
    // effective data classification was escalated during the batch, check
    // that each tool's channel class still permits the new level.
    let outcomes: Vec<ToolCallOutcome> = if mode == ToolExecutionMode::Parallel {
        let final_dc = context.effective_data_class();
        outcomes
            .into_iter()
            .map(|mut o| {
                if let Some(channel_class) = o.channel_class {
                    if !channel_class.allows(final_dc) {
                        tracing::warn!(
                            tool_id = %o.tool_id,
                            channel_class = ?channel_class,
                            effective_dc = ?final_dc,
                            "redacting tool result: classification escalated during parallel batch"
                        );
                        o.output = "Tool result redacted: session data classification was \
                                    escalated during parallel execution, and this tool's channel \
                                    class no longer permits the current classification level."
                            .to_string();
                    }
                }
                o
            })
            .collect()
    } else {
        outcomes
    };

    let mut tool_results = String::new();
    let mut journal_tool_calls = Vec::new();
    for o in outcomes {
        let capped = cap_tool_output(&o.output);
        let safe = hive_contracts::prompt_sanitize::escape_prompt_tags(&capped);
        tool_results.push_str(&format!(
            "\n\n<tool_call>\n{{\"tool\": \"{}\", \"input\": {}}}\n</tool_call>\n<tool_result>\n{}\n</tool_result>",
            o.tool_id, o.input_str, safe
        ));
        journal_tool_calls.push(JournalToolCall {
            tool_id: o.tool_id,
            input: o.input_str,
            output: cap_tool_output(&o.output),
        });
    }
    (tool_results, journal_tool_calls)
}

async fn run_single_tool_call(
    tool_call: &ToolCall,
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> ToolCallOutcome {
    let input_str =
        serde_json::to_string(&tool_call.input).unwrap_or_else(|_| "<unserializable>".to_string());

    if let Some(tx) = event_tx {
        if tx
            .try_send(LoopEvent::ToolCallStart {
                tool_id: tool_call.tool_id.clone(),
                input: input_str.clone(),
            })
            .is_err()
        {
            tracing::warn!(tool_id = %tool_call.tool_id, "failed to send ToolCallStart event");
        }
    }

    tracing::debug!(
        tool_id = %tool_call.tool_id,
        effective_dc_before = %context.effective_data_class(),
        "run_single_tool_call: starting"
    );

    let (output, is_error) = match execute_tool_call(
        context,
        tool_call.clone(),
        middleware,
        event_tx,
        interaction_gate,
        assistant_content,
    )
    .await
    {
        Ok(result) => {
            // Classification resolution and effective_data_class
            // escalation are handled by DataClassificationMiddleware
            // in its after_tool_result hook.
            let output = serde_json::to_string(&result.output)
                .unwrap_or_else(|_| "<unserializable>".to_string());
            (output, false)
        }
        Err(e) => (format!("ERROR: {e}"), true),
    };

    if let Some(tx) = event_tx {
        if tx
            .try_send(LoopEvent::ToolCallResult {
                tool_id: tool_call.tool_id.clone(),
                output: output.clone(),
                is_error,
            })
            .is_err()
        {
            tracing::warn!(tool_id = %tool_call.tool_id, "failed to send ToolCallResult event");
        }
    }

    let channel_class =
        context.tools_ctx.tools.get(&tool_call.tool_id).map(|t| t.definition().channel_class);

    ToolCallOutcome {
        tool_id: tool_call.tool_id.clone(),
        input_str,
        output,
        is_error,
        channel_class,
    }
}

async fn execute_tools_sequential(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    stop_on_error: bool,
    assistant_content: Option<&str>,
) -> Vec<ToolCallOutcome> {
    let mut outcomes = Vec::with_capacity(calls.len());
    for tool_call in calls {
        let outcome = run_single_tool_call(
            tool_call,
            context,
            middleware,
            event_tx,
            interaction_gate,
            assistant_content,
        )
        .await;
        let failed = outcome.is_error;
        outcomes.push(outcome);
        if stop_on_error && failed {
            break;
        }
    }
    outcomes
}

async fn execute_tools_parallel(
    calls: &[ToolCall],
    context: &LoopContext,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Vec<ToolCallOutcome> {
    // Cap concurrency to avoid resource exhaustion from large batches.
    const MAX_CONCURRENT_TOOLS: usize = 10;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_TOOLS));

    let futures: Vec<_> = calls
        .iter()
        .map(|tool_call| {
            let sem = Arc::clone(&semaphore);
            async move {
                let _permit = sem.acquire().await.expect("semaphore closed unexpectedly");
                run_single_tool_call(
                    tool_call,
                    context,
                    middleware,
                    event_tx,
                    interaction_gate,
                    assistant_content,
                )
                .await
            }
        })
        .collect();
    futures_util::future::join_all(futures).await
}

async fn execute_tool_call(
    context: &LoopContext,
    call: ToolCall,
    middleware: &[Arc<dyn LoopMiddleware>],
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Result<ToolResult, LoopError> {
    let mut call = call;
    for hook in middleware {
        call = hook.before_tool_call(context, call)?;
    }

    let tool = context
        .tools()
        .get(&call.tool_id)
        .ok_or_else(|| LoopError::ToolUnavailable { tool_id: call.tool_id.clone() })?;
    let definition = tool.definition();

    // Normalize the tool_id to the canonical registry ID so that
    // permission checks, events, and logging use the real name
    // (e.g. `shell.execute` instead of `shell_execute`).
    call.tool_id = definition.id.clone();

    // --- Session permission check (before tool definition approval) ---
    let workspace_str = context.workspace_path().map(|p| p.to_string_lossy().to_string());
    let resource = infer_scope_with_workspace(&call.tool_id, &call.input, workspace_str.as_deref());
    let needs_approval = {
        let perms = context.security.permissions.lock();
        let rules_summary: Vec<String> = perms
            .rules
            .iter()
            .map(|r| format!("({} | {} | {:?})", r.tool_pattern, r.scope, r.decision))
            .collect();
        let approval =
            crate::tool_policy::resolve_tool_approval(&call.tool_id, &resource, definition, &perms);
        tracing::info!(
            tool_id = %call.tool_id,
            resource = %resource,
            rule_count = perms.rules.len(),
            rules = ?rules_summary,
            decision = ?approval,
            "session permission check"
        );
        match approval {
            crate::tool_policy::ResolvedApproval::Auto => false,
            crate::tool_policy::ResolvedApproval::Deny { reason } => {
                return Err(LoopError::ToolDenied { tool_id: call.tool_id.clone(), reason });
            }
            crate::tool_policy::ResolvedApproval::Ask => true,
        }
    };

    // ── Connector destination-rule enforcement ─────────────────────────
    // Evaluate the per-connector destination rules (Deny/Ask/Auto) for
    // comm tools.  Deny blocks immediately; Ask forces approval even if
    // the session-level / tool-level decision was Auto.
    let (needs_approval, connector_rule_reason) = if call.tool_id.starts_with("comm.send") {
        if let Some(ref svc) = context.security.connector_service {
            let connector_id = call.input.get("connector_id").and_then(|v| v.as_str());
            let to = call.input.get("to").and_then(|v| v.as_str());
            if let (Some(cid), Some(dest)) = (connector_id, to) {
                match svc.resolve_destination_approval(cid, dest) {
                    Some(ToolApproval::Deny) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: DENY"
                        );
                        return Err(LoopError::ToolDenied {
                            tool_id: call.tool_id.clone(),
                            reason: format!(
                                "destination '{dest}' is denied by a connector rule on '{cid}'"
                            ),
                        });
                    }
                    Some(ToolApproval::Ask) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: ASK"
                        );
                        (
                            true,
                            Some(format!(
                                "Connector rule on '{}' requires approval to send to '{}'.",
                                cid, dest
                            )),
                        )
                    }
                    Some(ToolApproval::Auto) => {
                        tracing::info!(
                            connector_id = cid,
                            destination = dest,
                            "connector destination rule: AUTO"
                        );
                        (needs_approval, None)
                    }
                    None => (needs_approval, None),
                }
            } else {
                (needs_approval, None)
            }
        } else {
            (needs_approval, None)
        }
    } else {
        (needs_approval, None)
    };

    // ── Channel-class check ────────────────────────────────────────────
    // The hard-deny case (violation + no approval path) is handled by
    // DataClassificationMiddleware::before_tool_call.  Here we only need
    // to detect the violation to modify the approval dialog's reason text.
    let effective_dc = context.effective_data_class();
    let channel_violation = !definition.channel_class.allows(effective_dc);

    if needs_approval || channel_violation {
        let reason = if channel_violation {
            format!(
                "Tool '{}' operates on {:?} channel but data is classified as {:?}. Approve to proceed anyway.",
                call.tool_id, definition.channel_class, effective_dc
            )
        } else if let Some(ref cr_reason) = connector_rule_reason {
            cr_reason.clone()
        } else {
            format!("Tool '{}' requires user approval before execution.", call.tool_id)
        };

        if let (Some(tx), Some(gate)) = (event_tx.as_ref(), interaction_gate) {
            let request_id = format!("approval-{}-{}", call.tool_id, uuid::Uuid::new_v4());
            let input_str = serde_json::to_string(&call.input).unwrap_or_default();

            let kind = InteractionKind::ToolApproval {
                tool_id: call.tool_id.clone(),
                input: input_str,
                reason: reason.clone(),
                inferred_scope: Some(resource.clone()),
            };
            let rx = gate.create_request(request_id.clone(), kind.clone());
            if tx
                .send(LoopEvent::UserInteractionRequired { request_id: request_id.clone(), kind })
                .await
                .is_err()
            {
                tracing::warn!(
                    "failed to send UserInteractionRequired event — loop receiver dropped"
                );
            }

            match rx.await {
                Ok(UserInteractionResponse {
                    payload: InteractionResponsePayload::ToolApproval { approved: true, .. },
                    ..
                }) => { /* approved, continue to execute */ }
                _ => {
                    return Err(LoopError::ToolDenied {
                        tool_id: call.tool_id.clone(),
                        reason: "User denied the tool execution".to_string(),
                    });
                }
            }
        } else {
            return Err(LoopError::ToolDenied { tool_id: call.tool_id.clone(), reason });
        }
    }

    // ── Connector output-class enforcement ─────────────────────────────
    // For comm.send_external_message, resolve the connector's output class
    // and compare against the effective session data-class.  If the connector
    // cannot handle the data sensitivity, trigger an approval dialog.
    // This must remain inline because it uses the async interaction gate.
    if call.tool_id == "comm.send_external_message" {
        if let Some(ref svc) = context.security.connector_service {
            let connector_id = call.input.get("connector_id").and_then(|v| v.as_str());
            let to = call.input.get("to").and_then(|v| v.as_str());
            if let (Some(cid), Some(dest)) = (connector_id, to) {
                let output_class =
                    svc.resolve_output_class(cid, dest).unwrap_or(DataClass::Internal);
                if output_class < effective_dc {
                    let reason = format!(
                        "Connector '{}' is classified as {} (outbound) but this session \
                         contains {} data. Approve to send anyway.",
                        cid, output_class, effective_dc
                    );

                    if let (Some(tx), Some(gate)) = (event_tx.as_ref(), interaction_gate) {
                        let request_id =
                            format!("approval-class-{}-{}", call.tool_id, uuid::Uuid::new_v4());
                        let input_str = serde_json::to_string(&call.input).unwrap_or_default();
                        let kind = InteractionKind::ToolApproval {
                            tool_id: call.tool_id.clone(),
                            input: input_str,
                            reason: reason.clone(),
                            inferred_scope: None,
                        };
                        let rx = gate.create_request(request_id.clone(), kind.clone());
                        if tx
                            .send(LoopEvent::UserInteractionRequired { request_id, kind })
                            .await
                            .is_err()
                        {
                            tracing::warn!("failed to send UserInteractionRequired event — loop receiver dropped");
                        }
                        match rx.await {
                            Ok(UserInteractionResponse {
                                payload:
                                    InteractionResponsePayload::ToolApproval { approved: true, .. },
                                ..
                            }) => { /* user approved the override */ }
                            _ => {
                                return Err(LoopError::ToolDenied {
                                    tool_id: call.tool_id.clone(),
                                    reason: format!(
                                        "Blocked: cannot send {} data through {} connector '{}'",
                                        effective_dc, output_class, cid
                                    ),
                                });
                            }
                        }
                    } else {
                        return Err(LoopError::ToolDenied {
                            tool_id: call.tool_id.clone(),
                            reason,
                        });
                    }
                }
            }
        }
    }

    // Handle built-in tools that the loop intercepts directly.
    if call.tool_id == "core.ask_user" {
        return handle_question_tool(&call, event_tx, interaction_gate, assistant_content).await;
    }
    if call.tool_id == "core.activate_skill" {
        return handle_activate_skill(&call, context).await;
    }
    if call.tool_id == "core.spawn_agent" {
        return handle_spawn_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.list_agents" {
        return handle_list_agents_tool(&call, context).await;
    }
    if call.tool_id == "core.get_agent_result" {
        return handle_get_agent_result_tool(&call, context).await;
    }
    if call.tool_id == "core.wait_for_agent" {
        return handle_wait_for_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.list_personas" {
        return handle_list_personas_tool(&call, context).await;
    }
    if call.tool_id == "core.kill_agent" {
        return handle_kill_agent_tool(&call, context).await;
    }
    if call.tool_id == "core.signal_agent" {
        return handle_signal_agent_tool(&call, context).await;
    }
    if call.tool_id == "knowledge.query" {
        return handle_knowledge_query_tool(&call, context).await;
    }

    // ── Shadow mode interception ──────────────────────────────────────
    // When shadow_mode is active, intercept external side-effecting tools.
    // Built-in orchestration tools (core.*, knowledge.*) are handled above
    // and always pass through.  Read-only tools also pass through so the
    // agent can reason over real data.
    if context.security.shadow_mode {
        let is_read_only = definition.annotations.read_only_hint == Some(true)
            || !definition.side_effects;
        if !is_read_only {
            tracing::info!(
                tool_id = %call.tool_id,
                "shadow mode: intercepting side-effecting tool call"
            );
            // Emit an interception event so callers (e.g. workflow test
            // runner) can record what the agent *would* have done.
            if let Some(tx) = event_tx {
                let input_str = serde_json::to_string(&call.input)
                    .unwrap_or_else(|_| "<unserializable>".to_string());
                let _ = tx.try_send(LoopEvent::ToolCallIntercepted {
                    tool_id: call.tool_id.clone(),
                    input: input_str,
                });
            }
            // Return a clean success so the agent continues normally.
            // Do NOT include "shadow" or explanatory messages — the LLM
            // would interpret them as partial failures and retry/re-ask.
            let synthetic_output = serde_json::json!({
                "success": true,
            });
            let result = hive_tools::ToolResult {
                output: synthetic_output,
                data_class: DataClass::Internal,
            };
            // Still run after_tool_result middleware so classification and
            // other hooks see the synthetic result.
            let mut result = result;
            for hook in middleware {
                result = hook.after_tool_result(
                    context,
                    &call.tool_id,
                    Some(&call.input),
                    result,
                )?;
            }
            return Ok(result);
        }
    }

    // Snapshot the tool input before it's moved into execute()
    let tool_input_snapshot = call.input.clone();

    // Inform the tool of the effective session data-class so that
    // output-channel enforcement can compare the connector's class against
    // the true high-water mark of data the session has touched.
    tool.set_session_data_class(context.effective_data_class());

    let result = if let Some(ref token) = context.cancellation_token {
        tokio::select! {
            biased;
            _ = token.cancelled() => {
                return Err(LoopError::Cancelled);
            }
            result = tool.execute(call.input) => {
                result.map_err(|error| {
                    LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail: error.to_string() }
                })?
            }
        }
    } else {
        tool.execute(call.input).await.map_err(|error| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: error.to_string(),
        })?
    };

    // after_tool_result hooks handle classification resolution and
    // effective_data_class escalation (via DataClassificationMiddleware).
    let mut result = result;
    for hook in middleware {
        result =
            hook.after_tool_result(context, &call.tool_id, Some(&tool_input_snapshot), result)?;
    }

    Ok(result)
}

/// Handle the built-in `core.ask_user` tool by emitting a user interaction
/// event and blocking until the user responds.
async fn handle_question_tool(
    call: &ToolCall,
    event_tx: Option<&tokio::sync::mpsc::Sender<LoopEvent>>,
    interaction_gate: Option<&UserInteractionGate>,
    assistant_content: Option<&str>,
) -> Result<ToolResult, LoopError> {
    let text = call.input.get("question").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let choices: Vec<String> = call
        .input
        .get("choices")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let allow_freeform = call.input.get("allow_freeform").and_then(|v| v.as_bool()).unwrap_or(true);
    let multi_select = call.input.get("multi_select").and_then(|v| v.as_bool()).unwrap_or(false);
    let message = assistant_content.filter(|s| !s.is_empty()).map(String::from);

    if let (Some(tx), Some(gate)) = (event_tx, interaction_gate) {
        let request_id = format!("question-{}", uuid::Uuid::new_v4());
        let kind = InteractionKind::Question {
            text: text.clone(),
            choices: choices.clone(),
            allow_freeform,
            multi_select,
            message,
        };
        // Create the gate request FIRST so that the interaction is
        // queryable when the event triggers a snapshot rebuild (e.g.
        // the interactions SSE calls list_pending()).
        let rx = gate.create_request(request_id.clone(), kind.clone());

        if tx.send(LoopEvent::UserInteractionRequired { request_id, kind }).await.is_err() {
            tracing::warn!("failed to send UserInteractionRequired event — loop receiver dropped");
        }
        match rx.await {
            Ok(UserInteractionResponse {
                payload:
                    InteractionResponsePayload::Answer {
                        selected_choice,
                        selected_choices,
                        text: answer_text,
                    },
                ..
            }) => {
                // Build the answer string for the LLM
                let answer = if let Some(ref indices) = selected_choices {
                    // Multi-select: join all selected choice labels
                    let labels: Vec<String> = indices
                        .iter()
                        .map(|&idx| {
                            choices.get(idx).cloned().unwrap_or_else(|| format!("Choice {idx}"))
                        })
                        .collect();
                    if labels.is_empty() {
                        "(no choices selected)".to_string()
                    } else {
                        labels.join(", ")
                    }
                } else if let Some(idx) = selected_choice {
                    choices.get(idx).cloned().unwrap_or_else(|| format!("Choice {idx}"))
                } else if let Some(ref t) = answer_text {
                    t.clone()
                } else {
                    "(no response)".to_string()
                };
                Ok(ToolResult {
                    output: serde_json::json!({ "answer": answer }),
                    data_class: DataClass::Internal,
                })
            }
            _ => Ok(ToolResult {
                output: serde_json::json!({ "answer": "(user did not respond)" }),
                data_class: DataClass::Internal,
            }),
        }
    } else {
        Err(LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "Question tool requires an active UI connection".to_string(),
        })
    }
}

async fn handle_activate_skill(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let name = call.input.get("name").and_then(|value| value.as_str()).ok_or_else(|| {
        LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'name' parameter".to_string(),
        }
    })?;

    if let Some(ref catalog) = context.tools_ctx.skill_catalog {
        match catalog.activate(name) {
            Some(result) => {
                let mut content = result.content;

                // Stage skill resources into the workspace so the model can
                // access them via the sandboxed filesystem tools.
                if let (Some(source_dir), Some(workspace)) =
                    (&result.source_dir, context.workspace_path())
                {
                    let target = workspace.join(".skills").join(name);
                    match hive_skills::stage_skill_resources(source_dir, &target) {
                        Ok(_) => {
                            let abs_str = source_dir.to_string_lossy();
                            let relative = format!(".skills/{name}");
                            content = content.replace(abs_str.as_ref(), &relative);
                        }
                        Err(e) => {
                            tracing::warn!(
                                skill = name,
                                error = %e,
                                "failed to stage skill resources into workspace"
                            );
                        }
                    }
                }

                Ok(ToolResult {
                    output: serde_json::json!({ "content": content }),
                    data_class: hive_classification::DataClass::Internal,
                })
            }
            None => Err(LoopError::ToolExecutionFailed {
                tool_id: call.tool_id.clone(),
                detail: format!("skill '{name}' is not installed or enabled"),
            }),
        }
    } else {
        Err(LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "no skill catalog available".to_string(),
        })
    }
}

async fn handle_spawn_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    // Accept "persona" (preferred) or "agent_name" (backward compat)
    let persona_name = call
        .input
        .get("persona")
        .or_else(|| call.input.get("agent_name"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let friendly_name = call
        .input
        .get("friendly_name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from);
    let keep_alive = match call.input.get("mode").and_then(|v| v.as_str()) {
        Some("idle_after_task") | Some("continuous") => true,
        // Also support legacy "keep_alive" boolean for backward compat.
        _ => call.input.get("keep_alive").and_then(|v| v.as_bool()).unwrap_or(false),
    };
    let task = call
        .input
        .get("task")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'task' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let definition = persona_name
        .and_then(|name| resolve_persona_by_name(name, context.personas()))
        .cloned()
        .unwrap_or_else(|| {
            context
                .personas()
                .iter()
                .find(|d| d.id == "system/general")
                .cloned()
                .unwrap_or_else(Persona::default_persona)
        });

    let from = current_agent_sender_id(context);
    let parent_model = context.routing_decision().map(|d| d.selected.clone());
    let parent_workspace = context.workspace_path().map(PathBuf::from);
    let agent_id = orchestrator
        .spawn_agent(
            definition,
            task.to_string(),
            from,
            friendly_name,
            context.effective_data_class(),
            parent_model,
            keep_alive,
            parent_workspace,
        )
        .await
        .map_err(|detail| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail,
        })?;

    Ok(ToolResult {
        output: serde_json::json!({ "agent_id": agent_id }),
        data_class: DataClass::Internal,
    })
}
async fn handle_list_agents_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agents = orchestrator.list_agents().await.map_err(|detail| {
        LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
    })?;

    let entries: Vec<serde_json::Value> = agents
        .into_iter()
        .map(|(id, name, description, status, result)| {
            let mut entry = serde_json::json!({
                "id": id,
                "name": name,
                "description": description,
                "status": status,
            });
            if let Some(ref r) = result {
                let truncated =
                    if r.len() > 200 { format!("{}…", truncate_str(r, 200)) } else { r.clone() };
                entry["result_preview"] = serde_json::json!(truncated);
            }
            entry
        })
        .collect();

    Ok(ToolResult {
        output: serde_json::json!({ "agents": entries }),
        data_class: DataClass::Internal,
    })
}

async fn handle_get_agent_result_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = call.input["agent_id"]
        .as_str()
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required parameter: agent_id".to_string(),
        })?
        .to_string();

    let (status, result) =
        orchestrator.get_agent_result(agent_id.clone()).await.map_err(|detail| {
            LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
        })?;

    let mut output = serde_json::json!({
        "agent_id": agent_id,
        "status": status,
    });
    match result {
        Some(r) => output["result"] = serde_json::json!(r),
        None => output["result"] = serde_json::json!(null),
    }

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

async fn handle_wait_for_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = call.input["agent_id"]
        .as_str()
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required parameter: agent_id".to_string(),
        })?
        .to_string();

    let timeout_secs = call.input.get("timeout_secs").and_then(|v| v.as_u64());

    // Race the actual wait against the preempt signal so that a new user
    // message can interrupt a long wait on a sub-agent (e.g. one that is
    // blocked on a question).
    let preempt = context.preempt_signal.clone();
    let wait_fut = orchestrator.wait_for_agent(agent_id.clone(), timeout_secs);
    tokio::pin!(wait_fut);

    let (status, result) = tokio::select! {
        res = &mut wait_fut => {
            res.map_err(|detail| {
                LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
            })?
        }
        _ = poll_preempt_signal(preempt) => {
            ("preempted".to_string(), Some("A new user message arrived; the wait was interrupted. The sub-agent is still running.".to_string()))
        }
    };

    let mut output = serde_json::json!({
        "agent_id": agent_id,
        "status": status,
    });
    match result {
        Some(r) => output["result"] = serde_json::json!(r),
        None => output["result"] = serde_json::json!(null),
    }

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

/// Poll an `AtomicBool` preempt signal at short intervals.
/// Resolves when the signal is set to `true`, or never if `signal` is `None`.
async fn poll_preempt_signal(signal: Option<Arc<AtomicBool>>) {
    match signal {
        Some(sig) => loop {
            if sig.load(AtomicOrdering::Acquire) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        },
        None => std::future::pending::<()>().await,
    }
}

async fn handle_list_personas_tool(
    _call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let entries: Vec<serde_json::Value> = context
        .personas()
        .iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "description": p.description,
            })
        })
        .collect();

    Ok(ToolResult {
        output: serde_json::json!({ "personas": entries }),
        data_class: DataClass::Internal,
    })
}

async fn handle_kill_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let agent_id = call
        .input
        .get("agent_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'agent_id' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    // Access control: only the direct parent of an agent can kill it.
    // Session-level callers (no current_agent_id) are always allowed.
    // Bot/service-prefixed targets cannot be killed from agent tools.
    if agent_id.starts_with("bot:") || agent_id.starts_with("service:") {
        return Ok(ToolResult {
            output: serde_json::json!({
                "error": "Access denied: cannot kill global bot/service agents."
            }),
            data_class: DataClass::Internal,
        });
    }
    if let Some(caller_id) = context.current_agent_id() {
        // Check that the caller is the direct parent of the target.
        match orchestrator.get_agent_parent(agent_id.to_string()).await {
            Ok(Some(parent)) if parent == caller_id => { /* allowed — caller is parent */ }
            Ok(_) => {
                return Ok(ToolResult {
                    output: serde_json::json!({
                        "error": "Access denied: you can only kill agents that you spawned (your direct children)."
                    }),
                    data_class: DataClass::Internal,
                });
            }
            Err(e) => {
                return Ok(ToolResult {
                    output: serde_json::json!({ "error": format!("Cannot verify agent relationship: {e}") }),
                    data_class: DataClass::Internal,
                });
            }
        }
    }

    orchestrator.kill_agent(agent_id.to_string()).await.map_err(|detail| {
        LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
    })?;

    Ok(ToolResult {
        output: serde_json::json!({ "killed": true }),
        data_class: DataClass::Internal,
    })
}

async fn handle_signal_agent_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let requested_target = call
        .input
        .get("agent_id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'agent_id' parameter".to_string(),
        })?;
    let message = call
        .input
        .get("content")
        .or_else(|| call.input.get("message"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "missing required 'content' parameter".to_string(),
        })?;

    let orchestrator =
        context.agent_orchestrator().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "agent orchestration is not available in this context".to_string(),
        })?;

    let agent_id = if requested_target == "parent" {
        // "parent" resolves to the parent agent, or to "session" if spawned from the chat session
        context.parent_agent_id().map(|s| s.to_string()).unwrap_or_else(|| "session".to_string())
    } else {
        requested_target.to_string()
    };

    let from = current_agent_sender_id(context).unwrap_or_else(|| "unknown".to_string());

    // Access control: validate that the caller can message the target.
    // Skip for session-level callers, "session" target, and bot/service targets.
    if agent_id != "session" && !agent_id.starts_with("bot:") && !agent_id.starts_with("service:") {
        if let Some(caller_id) = context.current_agent_id() {
            if let Err(reason) =
                check_agent_family(orchestrator.as_ref(), caller_id, &agent_id).await
            {
                return Ok(ToolResult {
                    output: serde_json::json!({
                        "error": format!("Access denied: {reason}. You can only message your parent, children, or sibling agents.")
                    }),
                    data_class: DataClass::Internal,
                });
            }
        }
    }

    if agent_id == "session" {
        // For one-shot agents, only allow one message to the session.
        if !context.keep_alive() && context.session_messaged().load(AtomicOrdering::SeqCst) {
            return Ok(ToolResult {
                output: serde_json::json!({
                    "error": "Signal already delivered. You are a one-shot agent — \
                              do not signal again. Produce your final summary now."
                }),
                data_class: DataClass::Internal,
            });
        }
        // Route message back to the parent chat session
        orchestrator.message_session(message.to_string(), from).await.map_err(|detail| {
            LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail }
        })?;
        // Mark as messaged only after successful delivery so transient
        // failures don't permanently block retry attempts.
        if !context.keep_alive() {
            context.session_messaged().store(true, AtomicOrdering::SeqCst);
        }
    } else {
        orchestrator.message_agent(agent_id.clone(), message.to_string(), from).await.map_err(
            |detail| LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail },
        )?;
    }

    let output = if agent_id == "session" && !context.keep_alive() {
        serde_json::json!({
            "agent_id": agent_id,
            "delivered": true,
            "hint": "Signal delivered. Your task is complete — produce a brief final summary and stop."
        })
    } else {
        serde_json::json!({ "agent_id": agent_id, "delivered": true })
    };

    Ok(ToolResult { output, data_class: DataClass::Internal })
}

/// Check whether `caller_id` has a family relationship with `target_id` within
/// the same supervisor. Family = parent, child, or sibling (same parent).
/// Returns `Ok(())` if allowed, `Err(reason)` if not.
///
/// Both parents are queried from the orchestrator (the authoritative source)
/// rather than relying on the loop context's `parent_agent_id`, which
/// represents the sender of the current message — not the spawn parent.
async fn check_agent_family(
    orchestrator: &dyn AgentOrchestrator,
    caller_id: &str,
    target_id: &str,
) -> Result<(), String> {
    let caller_parent = orchestrator.get_agent_parent(caller_id.to_string()).await?;
    let target_parent = orchestrator.get_agent_parent(target_id.to_string()).await?;

    // Target is the caller's parent?
    if caller_parent.as_deref() == Some(target_id) {
        return Ok(());
    }

    // Caller is the target's parent?
    if target_parent.as_deref() == Some(caller_id) {
        return Ok(());
    }

    // Siblings — share the same parent (both root-level or same parent agent).
    if caller_parent == target_parent {
        return Ok(());
    }

    Err(format!("agent '{caller_id}' has no family relationship with '{target_id}'"))
}

async fn handle_knowledge_query_tool(
    call: &ToolCall,
    context: &LoopContext,
) -> Result<ToolResult, LoopError> {
    let handler =
        context.knowledge_query_handler().ok_or_else(|| LoopError::ToolExecutionFailed {
            tool_id: call.tool_id.clone(),
            detail: "knowledge graph is not available in this context".to_string(),
        })?;

    handler
        .handle_query(call.input.clone())
        .await
        .map_err(|detail| LoopError::ToolExecutionFailed { tool_id: call.tool_id.clone(), detail })
}

fn resolve_persona_by_name<'a>(
    agent_name: &str,
    definitions: &'a [Persona],
) -> Option<&'a Persona> {
    definitions
        .iter()
        .find(|definition| definition.id == agent_name || definition.name == agent_name)
        .or_else(|| {
            definitions.iter().find(|definition| {
                definition.id.eq_ignore_ascii_case(agent_name)
                    || definition.name.eq_ignore_ascii_case(agent_name)
            })
        })
}

fn current_agent_sender_id(context: &LoopContext) -> Option<String> {
    context.current_agent_id().map(|s| s.to_string())
}

/// Parse **all** tool calls from model text output. Handles many formats
/// that small/local models produce:
///   - `<tool_call>{ JSON }</tool_call>` XML blocks (multiple allowed)
///   - `<function_call>{ JSON }</function_call>` alternate XML
///   - ` ```json { JSON } ``` ` fenced code blocks
///   - ` ``` { JSON } ``` ` plain fenced code blocks
///
/// Accepts keys: tool / tool_id for the tool name.
/// Accepts keys: input / arguments for the arguments.
pub fn parse_tool_calls(content: &str) -> Vec<ToolCall> {
    let trimmed = content.trim();
    let mut calls = Vec::new();

    // 1. Try XML-style blocks (multiple occurrences)
    let xml_tags = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
    ];
    for (open, close) in &xml_tags {
        for block in extract_all_between(trimmed, open, close) {
            if let Some(call) = try_parse_tool_json(&block) {
                calls.push(call);
            }
        }
    }
    if !calls.is_empty() {
        return calls;
    }

    // 2. Try fenced code blocks (```json or ```)
    if let Some(block) = extract_fenced(trimmed, "```json", "```") {
        if let Some(call) = try_parse_tool_json(&block) {
            return vec![call];
        }
    }
    if let Some(block) = extract_fenced(trimmed, "```", "```") {
        let cleaned = strip_first_line_if_not_json(&block);
        if let Some(call) = try_parse_tool_json(&cleaned) {
            return vec![call];
        }
    }

    calls
}

/// Convenience wrapper — returns the first parsed tool call, if any.
pub fn parse_tool_call(content: &str) -> Option<ToolCall> {
    parse_tool_calls(content).into_iter().next()
}

/// Try to parse a JSON string as a tool call, accepting many key name variations.
fn try_parse_tool_json(candidate: &str) -> Option<ToolCall> {
    let value: Value = serde_json::from_str(candidate.trim())
        .map_err(|e| tracing::debug!("tool call JSON parse attempt failed: {e}"))
        .ok()?;
    let object = value.as_object()?;

    // Accept only canonical key names for the tool identifier
    let tool = object.get("tool").or_else(|| object.get("tool_id")).and_then(|v| v.as_str())?;

    // Accept only canonical key names for the arguments
    let input =
        object.get("input").or_else(|| object.get("arguments")).cloned().unwrap_or(Value::Null);

    Some(ToolCall { tool_id: tool.to_string(), input })
}

/// If the first line of a fenced block isn't JSON, strip it (language tag).
fn strip_first_line_if_not_json(block: &str) -> String {
    let trimmed = block.trim();
    if let Some(first_newline) = trimmed.find('\n') {
        let first_line = trimmed[..first_newline].trim();
        // If first line doesn't start with '{' or '[', it's likely a language tag
        if !first_line.starts_with('{') && !first_line.starts_with('[') {
            return trimmed[first_newline + 1..].trim().to_string();
        }
    }
    trimmed.to_string()
}

/// Extract the first balanced JSON object from text using brace counting.
#[allow(dead_code)]
fn extract_json_object(content: &str) -> Option<String> {
    let bytes = content.as_bytes();
    let mut start = None;
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, &b) in bytes.iter().enumerate() {
        if escape {
            escape = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if b == b'{' {
            if start.is_none() {
                start = Some(i);
            }
            depth += 1;
        } else if b == b'}' {
            depth -= 1;
            if depth == 0 {
                if let Some(s) = start {
                    return Some(content[s..=i].to_string());
                }
            }
        }
    }
    None
}

/// Extract ALL occurrences of text between `start` and `end` tags.
fn extract_all_between(content: &str, start: &str, end: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut search_from = 0;
    while search_from < content.len() {
        let Some(start_index) = content[search_from..].find(start) else {
            break;
        };
        let abs_start = search_from + start_index + start.len();
        let Some(end_index) = content[abs_start..].find(end) else {
            break;
        };
        let abs_end = abs_start + end_index;
        results.push(content[abs_start..abs_end].trim().to_string());
        search_from = abs_end + end.len();
    }
    results
}

/// Remove `<tool_call>…</tool_call>`, `<function_call>…</function_call>`,
/// `<tool_use>…</tool_use>`, and `<tool_result>…</tool_result>` XML blocks
/// from model text so they are not leaked into user-visible output.
pub fn strip_xml_tool_blocks(content: &str) -> String {
    let tags = [
        ("<tool_call>", "</tool_call>"),
        ("<function_call>", "</function_call>"),
        ("<tool_use>", "</tool_use>"),
        ("<tool_result>", "</tool_result>"),
    ];
    let mut result = content.to_string();
    for (open, close) in &tags {
        while let Some(start) = result.find(open) {
            if let Some(end_offset) = result[start..].find(close) {
                result.replace_range(start..start + end_offset + close.len(), "");
            } else {
                // Unclosed tag — remove from opening tag to end of string
                result.truncate(start);
                break;
            }
        }
    }
    result.trim().to_string()
}

fn extract_fenced(content: &str, start: &str, end: &str) -> Option<String> {
    let start_index = content.find(start)?;
    let end_index = content[start_index + start.len()..].find(end)?;
    let begin = start_index + start.len();
    let finish = start_index + start.len() + end_index;
    Some(content[begin..finish].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hive_classification::ChannelClass;
    use hive_contracts::{
        PermissionRule, Persona, ToolAnnotations, ToolApproval, ToolDefinition,
        WorkspaceClassification,
    };
    use hive_model::{ModelProvider, ModelSelection, ProviderDescriptor};
    use hive_tools::{
        CalculatorTool, FileSystemListTool, FileSystemReadTool, KillAgentTool, ListAgentsTool,
        ListPersonasTool, SignalAgentTool, SpawnAgentTool, Tool, ToolRegistry,
    };
    use serde_json::json;
    use std::collections::{BTreeMap, BTreeSet, VecDeque};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone)]
    struct TestProvider {
        descriptor: ProviderDescriptor,
        responses: Arc<Mutex<VecDeque<String>>>,
        prompts: Arc<Mutex<Vec<String>>>,
    }

    impl TestProvider {
        fn new(responses: Vec<String>) -> (Self, Arc<Mutex<Vec<String>>>) {
            let prompts = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    descriptor: ProviderDescriptor {
                        id: "test".to_string(),
                        name: None,
                        kind: hive_model::ProviderKind::Mock,
                        models: vec!["test-model".to_string()],
                        model_capabilities: BTreeMap::from([(
                            "test-model".to_string(),
                            BTreeSet::from([Capability::Chat]),
                        )]),
                        priority: 10,
                        available: true,
                    },
                    responses: Arc::new(Mutex::new(VecDeque::from(responses))),
                    prompts: Arc::clone(&prompts),
                },
                prompts,
            )
        }
    }

    #[derive(Default)]
    struct MockAgentOrchestrator {
        next_id: AtomicUsize,
        spawned: Mutex<Vec<(String, String, Option<String>)>>,
        messages: Mutex<Vec<(String, String, String)>>,
    }

    impl AgentOrchestrator for MockAgentOrchestrator {
        fn spawn_agent(
            &self,
            persona: Persona,
            task: String,
            from: Option<String>,
            _friendly_name: Option<String>,
            _data_class: hive_classification::DataClass,
            _parent_model: Option<hive_model::ModelSelection>,
            _keep_alive: bool,
            _workspace_path: Option<PathBuf>,
        ) -> BoxFuture<'_, Result<String, String>> {
            self.spawned.lock().unwrap().push((persona.id.clone(), task, from));
            let agent_id =
                format!("{}-{}", persona.id, self.next_id.fetch_add(1, Ordering::SeqCst) + 1);
            Box::pin(async move { Ok(agent_id) })
        }

        fn message_agent(
            &self,
            agent_id: String,
            message: String,
            from: String,
        ) -> BoxFuture<'_, Result<(), String>> {
            self.messages.lock().unwrap().push((agent_id, message, from));
            Box::pin(async move { Ok(()) })
        }

        fn list_agents(
            &self,
        ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>>
        {
            let spawned = self.spawned.lock().unwrap();
            let agents: Vec<_> = spawned
                .iter()
                .map(|(id, _, _)| {
                    (id.clone(), id.clone(), String::new(), "Running".to_string(), None)
                })
                .collect();
            Box::pin(async move { Ok(agents) })
        }

        fn get_agent_result(
            &self,
            agent_id: String,
        ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
            Box::pin(async move { Ok(("Done".to_string(), None)) })
        }

        fn kill_agent(&self, _agent_id: String) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async move { Ok(()) })
        }

        fn message_session(
            &self,
            _message: String,
            _from_agent_id: String,
        ) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async move { Ok(()) })
        }

        fn feedback_agent(
            &self,
            agent_id: String,
            message: String,
            from: String,
        ) -> BoxFuture<'_, Result<(), String>> {
            self.messages.lock().unwrap().push((agent_id, message, from));
            Box::pin(async move { Ok(()) })
        }

        fn get_agent_parent(
            &self,
            agent_id: String,
        ) -> BoxFuture<'_, Result<Option<String>, String>> {
            // The test context sets current_agent_id = "system/general" with
            // parent_agent_id = "parent-1". Return matching relationships so
            // check_agent_family access control passes.
            let parent = match agent_id.as_str() {
                "system/general" | "system/planner-1" => Some("parent-1".to_string()),
                _ => None,
            };
            Box::pin(async move { Ok(parent) })
        }
    }

    impl ModelProvider for TestProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn complete(
            &self,
            request: &CompletionRequest,
            selection: &ModelSelection,
        ) -> Result<CompletionResponse, anyhow::Error> {
            self.prompts.lock().unwrap().push(request.prompt.clone());
            let mut responses = self.responses.lock().unwrap();
            let response = responses.pop_front().ok_or_else(|| anyhow::anyhow!("no response"))?;
            Ok(CompletionResponse {
                provider_id: self.descriptor.id.clone(),
                model: selection.model.clone(),
                content: response,
                tool_calls: vec![],
            })
        }
    }

    #[tokio::test]
    async fn execute_tool_call_intercepts_agent_orchestration_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(SpawnAgentTool::default())).expect("register spawn tool");
        registry.register(Arc::new(SignalAgentTool::default())).expect("register signal tool");
        registry.register(Arc::new(ListAgentsTool::default())).expect("register list tool");
        registry
            .register(Arc::new(ListPersonasTool::default()))
            .expect("register list personas tool");
        registry.register(Arc::new(KillAgentTool::default())).expect("register kill tool");

        let orchestrator = Arc::new(MockAgentOrchestrator::default());
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-agents".to_string(),
                message_id: "msg-agents".to_string(),
                prompt: "coordinate work".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: Some(Persona::default_persona()),
                agent_orchestrator: Some(orchestrator.clone()),
                personas: vec![Persona {
                    id: "system/planner".to_string(),
                    name: "Planner".to_string(),
                    description: "Plans execution.".to_string(),
                    system_prompt: "Plan the work.".to_string(),
                    loop_strategy: ConfigLoopStrategy::React,
                    preferred_models: None,
                    allowed_tools: vec!["filesystem.read".to_string()],
                    mcp_servers: Vec::new(),
                    avatar: None,
                    color: None,
                    tool_execution_mode: ToolExecutionMode::default(),
                    context_map_strategy: hive_contracts::ContextMapStrategy::default(),
                    secondary_models: None,
                    archived: false,
                    bundled: false,
                    prompts: Default::default(),
                }],
                current_agent_id: Some("system/general".to_string()),
                parent_agent_id: Some("parent-1".to_string()),
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let spawn_result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "core.spawn_agent".to_string(),
                input: json!({ "agent_name": "Planner", "task": "Break down the task" }),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .expect("spawn tool result");
        assert_eq!(spawn_result.output["agent_id"], json!("system/planner-1"));

        let message_result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({ "agent_id": "parent", "content": "Done" }),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .expect("message tool result");
        assert_eq!(message_result.output["agent_id"], json!("parent-1"));
        assert_eq!(message_result.output["delivered"], json!(true));

        let spawned = orchestrator.spawned.lock().unwrap();
        assert_eq!(
            spawned.as_slice(),
            [(
                "system/planner".to_string(),
                "Break down the task".to_string(),
                Some("system/general".to_string())
            )]
        );
        drop(spawned);

        let messages = orchestrator.messages.lock().unwrap();
        assert_eq!(
            messages.as_slice(),
            [("parent-1".to_string(), "Done".to_string(), "system/general".to_string())]
        );
    }

    #[tokio::test]
    async fn react_executes_tool_call_and_responds() {
        let responses = vec![
            "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
                .to_string(),
            "done".to_string(),
        ];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
        let tools = Arc::new(registry);

        let executor = LoopExecutor::new(Arc::new(ReActStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-1".to_string(),
                message_id: "msg-1".to_string(),
                prompt: "Say hello".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools,
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert_eq!(result.content, "done");

        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        assert!(recorded[1].contains("<tool_result>"));
    }

    #[tokio::test]
    async fn sequential_returns_response_without_tool_calls() {
        let responses = vec!["Hello, world!".to_string()];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
        let tools = Arc::new(registry);

        let executor = LoopExecutor::new(Arc::new(SequentialStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-seq".to_string(),
                message_id: "msg-seq".to_string(),
                prompt: "What is 1+1?".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools,
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert_eq!(result.content, "Hello, world!");

        // Sequential should only call model once and never invoke tools
        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
    }

    #[tokio::test]
    async fn sequential_ignores_tool_call_in_response() {
        // Even if model returns a tool_call block, Sequential ignores it
        let responses =
            vec!["<tool_call>{\"tool\":\"core.echo\",\"input\":{\"value\":\"hi\"}}</tool_call>"
                .to_string()];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let tools = Arc::new(ToolRegistry::new());
        let executor = LoopExecutor::new(Arc::new(SequentialStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-seq2".to_string(),
                message_id: "msg-seq2".to_string(),
                prompt: "echo hi".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        // The raw content is returned as-is, no tool execution
        assert!(result.content.contains("tool_call"));
        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 1);
    }

    #[tokio::test]
    async fn loop_context_strategy_overrides_executor_default() {
        let responses = vec!["Hello from sequential".to_string()];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let executor = LoopExecutor::new(Arc::new(ReActStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-override".to_string(),
                message_id: "msg-override".to_string(),
                prompt: "Ignore tools".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: Some(ConfigLoopStrategy::Sequential),
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: Some(Persona::default_persona()),
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert_eq!(result.content, "Hello from sequential");
        assert_eq!(prompts.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn plan_then_execute_with_tool_calls() {
        let responses = vec![
            // Phase 1: plan
            "1. Calculate 1+1\n2. Summarize".to_string(),
            // Phase 2 step 1: tool call
            "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
                .to_string(),
            // Phase 2 step 1 continued: final answer for step
            "calculated 2".to_string(),
            // Phase 2 step 2: final answer (no tool)
            "summary complete".to_string(),
        ];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register calculator tool");
        let tools = Arc::new(registry);

        let executor = LoopExecutor::new(Arc::new(PlanThenExecuteStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-plan".to_string(),
                message_id: "msg-plan".to_string(),
                prompt: "Calculate 1+1 then summarize".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools,
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert_eq!(result.content, "summary complete");

        let recorded = prompts.lock().unwrap();
        // 1 plan call + 2 model calls for step 1 (tool call + answer) + 1 for step 2 = 4
        assert_eq!(recorded.len(), 4);
        // Second call should contain the step prompt
        assert!(recorded[1].contains("Current step"));
    }

    #[tokio::test]
    async fn plan_then_execute_no_plan_returns_response() {
        // If the model doesn't output a parseable plan, return the response as-is
        let responses = vec!["Just a plain answer without numbered steps".to_string()];
        let (provider, _prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let tools = Arc::new(ToolRegistry::new());
        let executor = LoopExecutor::new(Arc::new(PlanThenExecuteStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-plan2".to_string(),
                message_id: "msg-plan2".to_string(),
                prompt: "Do something".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert_eq!(result.content, "Just a plain answer without numbered steps");
    }

    #[tokio::test]
    async fn execute_tool_call_applies_workspace_classification_to_file_reads() {
        let workspace_root =
            std::env::temp_dir().join(format!("hive-loop-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace_root).expect("create temp workspace");
        std::fs::write(workspace_root.join("secret.txt"), "classified").expect("write temp file");

        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FileSystemReadTool::new(workspace_root.clone())))
            .expect("register file read tool");

        let mut workspace_classification = WorkspaceClassification::new(DataClass::Public);
        workspace_classification.set_override("secret.txt", DataClass::Restricted);

        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-file-read".to_string(),
                message_id: "msg-file-read".to_string(),
                prompt: "Read the secret file".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: Some(Arc::new(workspace_classification)),
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        // The DataClassificationMiddleware resolves workspace classification
        // in its after_tool_result hook.
        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(None));

        let result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({ "path": "secret.txt" }),
            },
            &[classification_mw],
            None,
            None,
            None,
        )
        .await
        .expect("tool call succeeds");

        assert_eq!(result.data_class, DataClass::Restricted);

        std::fs::remove_dir_all(&workspace_root).expect("cleanup temp workspace");
    }

    /// Integration test for the classification escalation flow.
    ///
    /// Scenario: workspace has two files — one Public, one Internal.
    /// Reading the public file should NOT escalate effective_data_class
    /// beyond Public.  Reading the internal file SHOULD escalate to Internal.
    /// Intermediate tool calls (filesystem.list) must not taint the session.
    #[tokio::test]
    async fn effective_data_class_only_escalates_from_classified_file_reads() {
        let workspace_root =
            std::env::temp_dir().join(format!("hive-loop-class-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace_root).expect("create temp workspace");
        std::fs::write(workspace_root.join("public.txt"), "hello world").expect("write public");
        std::fs::write(workspace_root.join("internal.txt"), "secret stuff")
            .expect("write internal");

        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(FileSystemReadTool::new(workspace_root.clone())))
            .expect("register read");
        registry
            .register(Arc::new(FileSystemListTool::new(workspace_root.clone())))
            .expect("register list");

        let mut wc = WorkspaceClassification::new(DataClass::Internal);
        wc.set_override("public.txt", DataClass::Public);
        wc.set_override("internal.txt", DataClass::Internal);

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-class-test".to_string(),
                message_id: "msg-class-test".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: Some(Arc::new(wc)),
                effective_data_class: effective_dc.clone(),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        // Classification middleware is needed for after_tool_result resolution.
        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(None));
        let mw = &[classification_mw];

        // Step 1: filesystem.list — should NOT escalate effective_data_class.
        // (filesystem.list hardcodes DataClass::Internal but classification is
        // not resolved for directory listings without a specific file match.)
        let outcome = run_single_tool_call(
            &ToolCall { tool_id: "filesystem.list".to_string(), input: json!({ "path": "." }) },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "filesystem.list should succeed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Public,
            "filesystem.list must NOT escalate effective_data_class"
        );

        // Step 2: Read the PUBLIC file — should resolve to Public, no escalation.
        let outcome = run_single_tool_call(
            &ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({ "path": "public.txt" }),
            },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "read public.txt should succeed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Public,
            "reading a Public file must keep effective_data_class at Public"
        );

        // Step 3: Read the INTERNAL file — should escalate to Internal.
        let outcome = run_single_tool_call(
            &ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({ "path": "internal.txt" }),
            },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "read internal.txt should succeed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Internal,
            "reading an Internal file must escalate effective_data_class to Internal"
        );

        std::fs::remove_dir_all(&workspace_root).expect("cleanup temp workspace");
    }

    /// Full end-to-end scenario: workspace with Public and Internal files,
    /// read only the public file, then send via a Public connector.
    /// The send MUST NOT be blocked by classification.
    #[tokio::test]
    async fn send_public_file_through_public_connector_is_not_blocked() {
        use hive_connectors::ConnectorServiceHandle;

        // Mock connector service that returns Public output-class
        struct MockConnectorSvc;
        impl ConnectorServiceHandle for MockConnectorSvc {
            fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
                Some(DataClass::Public)
            }
            fn resolve_destination_approval(
                &self,
                _cid: &str,
                _dest: &str,
            ) -> Option<hive_contracts::ToolApproval> {
                None
            }
        }

        // Minimal send tool that just succeeds (we don't care about actual send)
        struct FakeSendTool(ToolDefinition);
        impl FakeSendTool {
            fn new() -> Self {
                Self(ToolDefinition {
                    id: "comm.send_external_message".to_string(),
                    name: "Send".to_string(),
                    description: "mock".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    channel_class: ChannelClass::Public,
                    side_effects: true,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: "Send".to_string(),
                        read_only_hint: None,
                        destructive_hint: None,
                        idempotent_hint: None,
                        open_world_hint: None,
                    },
                })
            }
        }
        impl Tool for FakeSendTool {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }
            fn execute(
                &self,
                _input: Value,
            ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        output: json!({"status": "sent"}),
                        data_class: DataClass::Public,
                    })
                })
            }
        }

        let workspace_root =
            std::env::temp_dir().join(format!("hive-loop-e2e-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace_root).expect("create workspace");
        std::fs::write(workspace_root.join("public.txt"), "hello world").expect("write public");
        std::fs::write(workspace_root.join("internal.txt"), "secret stuff")
            .expect("write internal");

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FileSystemReadTool::new(workspace_root.clone()))).unwrap();
        registry.register(Arc::new(FileSystemListTool::new(workspace_root.clone()))).unwrap();
        registry.register(Arc::new(FakeSendTool::new())).unwrap();

        // Workspace default is Internal; public.txt overridden to Public
        let mut wc = WorkspaceClassification::new(DataClass::Internal);
        wc.set_override("public.txt", DataClass::Public);
        wc.set_override("internal.txt", DataClass::Internal);

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-e2e-send".to_string(),
                message_id: "msg-e2e-send".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: Some(Arc::new(wc)),
                effective_data_class: effective_dc.clone(),
                connector_service: Some(Arc::new(MockConnectorSvc)),
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
                Arc::new(MockConnectorSvc),
            )));
        let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

        // 1. Agent lists directory (should not taint session)
        let outcome = run_single_tool_call(
            &ToolCall { tool_id: "filesystem.list".to_string(), input: json!({"path": "."}) },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "list failed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Public,
            "filesystem.list must not escalate"
        );

        // 2. Agent reads public.txt
        let outcome = run_single_tool_call(
            &ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({"path": "public.txt"}),
            },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "read public.txt failed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Public,
            "public file read must not escalate"
        );

        // 3. Agent sends via comm.send_external_message through Public connector.
        //    This MUST succeed (not be blocked by classification).
        let send_result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "user@example.com",
                    "body": "hello world"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await
        .expect("send must NOT be blocked — session only has Public data");

        assert_eq!(send_result.output["status"], "sent");

        // 4. Now read the internal file — effective should escalate
        let outcome = run_single_tool_call(
            &ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({"path": "internal.txt"}),
            },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Internal,
            "internal file read must escalate"
        );

        // 5. Now sending through the Public connector SHOULD be blocked
        let send_result_2 = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "user@example.com",
                    "body": "secret stuff"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await;

        assert!(
            send_result_2.is_err(),
            "sending Internal data through Public connector must be blocked"
        );

        std::fs::remove_dir_all(&workspace_root).expect("cleanup");
    }

    /// When workspace classification uses the default (Public), reading any
    /// file must NOT escalate effective_data_class above Public — regardless
    /// of the hardcoded data_class on the tool result.
    #[tokio::test]
    async fn default_workspace_classification_does_not_taint_session() {
        use hive_connectors::ConnectorServiceHandle;

        struct MockConnectorSvc;
        impl ConnectorServiceHandle for MockConnectorSvc {
            fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
                Some(DataClass::Internal)
            }
            fn resolve_destination_approval(
                &self,
                _cid: &str,
                _dest: &str,
            ) -> Option<hive_contracts::ToolApproval> {
                None
            }
        }

        struct FakeSendTool(ToolDefinition);
        impl FakeSendTool {
            fn new() -> Self {
                Self(ToolDefinition {
                    id: "comm.send_external_message".to_string(),
                    name: "Send".to_string(),
                    description: "mock".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    channel_class: ChannelClass::Internal,
                    side_effects: true,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: "Send".to_string(),
                        read_only_hint: None,
                        destructive_hint: None,
                        idempotent_hint: None,
                        open_world_hint: None,
                    },
                })
            }
        }
        impl Tool for FakeSendTool {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }
            fn execute(
                &self,
                _input: Value,
            ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        output: json!({"status": "sent"}),
                        data_class: DataClass::Internal,
                    })
                })
            }
        }

        let workspace_root =
            std::env::temp_dir().join(format!("hive-loop-default-class-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workspace_root).expect("create workspace");
        std::fs::write(workspace_root.join("readme.txt"), "public content").expect("write file");

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FileSystemReadTool::new(workspace_root.clone()))).unwrap();
        registry.register(Arc::new(FakeSendTool::new())).unwrap();

        // Use DEFAULT workspace classification (no overrides at all).
        let wc = WorkspaceClassification::default();
        assert_eq!(wc.default, DataClass::Internal, "default must be Internal");

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-default-class".to_string(),
                message_id: "msg-default-class".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: Some(Arc::new(wc)),
                effective_data_class: effective_dc.clone(),
                connector_service: Some(Arc::new(MockConnectorSvc)),
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
                Arc::new(MockConnectorSvc),
            )));
        let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

        // Read a file with no override — should resolve to workspace default (Internal)
        let outcome = run_single_tool_call(
            &ToolCall {
                tool_id: "filesystem.read".to_string(),
                input: json!({"path": "readme.txt"}),
            },
            &context,
            mw,
            None,
            None,
            None,
        )
        .await;
        assert!(!outcome.is_error, "read failed: {}", outcome.output);
        assert_eq!(
            context.effective_data_class(),
            DataClass::Internal,
            "reading file with default Internal classification must NOT escalate beyond Internal"
        );

        // Send through Internal connector — must succeed
        let send_result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "user@example.com",
                    "body": "public content"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await
        .expect("send must succeed — session only has Internal data");

        assert_eq!(send_result.output["status"], "sent");

        std::fs::remove_dir_all(&workspace_root).expect("cleanup");
    }

    #[test]
    fn strategy_kind_build_returns_correct_type() {
        // Verify that build() returns a strategy that can be used with LoopExecutor
        let react = StrategyKind::ReAct.build();
        let sequential = StrategyKind::Sequential.build();
        let plan = StrategyKind::PlanThenExecute.build();

        // Each should produce a valid Arc<dyn LoopStrategy>
        let _executor_react = LoopExecutor::new(react);
        let _executor_seq = LoopExecutor::new(sequential);
        let _executor_plan = LoopExecutor::new(plan);
    }

    #[test]
    fn strategy_kind_equality() {
        assert_eq!(StrategyKind::ReAct, StrategyKind::ReAct);
        assert_eq!(StrategyKind::Sequential, StrategyKind::Sequential);
        assert_eq!(StrategyKind::PlanThenExecute, StrategyKind::PlanThenExecute);
        assert_ne!(StrategyKind::ReAct, StrategyKind::Sequential);
    }

    #[test]
    fn parse_plan_numbered_list() {
        let plan = "1. First step\n2. Second step\n3. Third step";
        let steps = PlanThenExecuteStrategy::parse_plan(plan);
        assert_eq!(steps, vec!["First step", "Second step", "Third step"]);
    }

    #[test]
    fn parse_plan_dash_list() {
        let plan = "- Step A\n- Step B";
        let steps = PlanThenExecuteStrategy::parse_plan(plan);
        assert_eq!(steps, vec!["Step A", "Step B"]);
    }

    #[test]
    fn parse_plan_parses_all_steps() {
        let lines: Vec<String> = (1..=15).map(|i| format!("{i}. Step {i}")).collect();
        let plan = lines.join("\n");
        let steps = PlanThenExecuteStrategy::parse_plan(&plan);
        // parse_plan returns all parsed steps; truncation to MAX_PLAN_STEPS happens in run()
        assert_eq!(steps.len(), 15);
    }

    // ── parse_tool_call tests ────────────────────────────────────

    #[test]
    fn test_parse_xml_tool_call() {
        let input = r#"<tool_call>
{"tool": "core.echo", "input": {"value": "hello"}}
</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
        assert_eq!(call.input["value"], "hello");
    }

    #[test]
    fn test_parse_tool_call_with_surrounding_text() {
        let input = r#"Sure, I'll echo that for you.

<tool_call>
{"tool": "core.echo", "input": {"value": "hello"}}
</tool_call>

Let me know if you need anything else."#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
    }

    #[test]
    fn test_parse_fenced_json_tool_call() {
        let input = "```json\n{\"tool\": \"filesystem.list\", \"input\": {\"path\": \".\"}}\n```";
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "filesystem.list");
    }

    #[test]
    fn test_parse_fenced_with_language_tag() {
        let input = "```tool_call\n{\"tool\": \"core.echo\", \"input\": {\"value\": \"hi\"}}\n```";
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "core.echo");
    }

    #[test]
    fn test_raw_json_in_prose_is_not_parsed() {
        // Raw JSON embedded in prose should NOT be parsed (injection risk)
        let input = r#"I'll use the echo tool: {"tool": "core.echo", "input": {"value": "test"}}"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_removed_key_aliases_no_longer_parsed() {
        // "name" and "params" are no longer accepted as key aliases
        let input =
            r#"<tool_call>{"name": "shell.execute", "params": {"command": "ls"}}</tool_call>"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_removed_function_key_alias_no_longer_parsed() {
        // "function" and "params" are no longer accepted
        let input = r#"<function_call>{"function": "http.request", "params": {"url": "https://example.com"}}</function_call>"#;
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_accepted_key_aliases_still_work() {
        // "tool" + "input" (canonical)
        let input1 = r#"<tool_call>{"tool": "core.echo", "input": {"value": "hi"}}</tool_call>"#;
        let call1 = parse_tool_call(input1).unwrap();
        assert_eq!(call1.tool_id, "core.echo");
        assert_eq!(call1.input["value"], "hi");

        // "tool_id" + "arguments"
        let input2 =
            r#"<tool_call>{"tool_id": "math.calc", "arguments": {"expr": "1+1"}}</tool_call>"#;
        let call2 = parse_tool_call(input2).unwrap();
        assert_eq!(call2.tool_id, "math.calc");
        assert_eq!(call2.input["expr"], "1+1");
    }

    #[test]
    fn test_parse_no_tool_call() {
        let input = "Hello! How can I help you today?";
        assert!(parse_tool_call(input).is_none());
    }

    #[test]
    fn test_parse_nested_json_in_input() {
        let input = r#"<tool_call>{"tool": "http.request", "input": {"method": "POST", "url": "https://api.example.com", "body": "{\"key\": \"value\"}"}}</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "http.request");
        assert_eq!(call.input["method"], "POST");
    }

    #[test]
    fn test_parse_tool_id_key() {
        let input = r#"<tool_call>{"tool_id": "math.calculate", "input": {"expression": "2+2"}}</tool_call>"#;
        let call = parse_tool_call(input).unwrap();
        assert_eq!(call.tool_id, "math.calculate");
    }

    fn make_batch_context(mode: ToolExecutionMode) -> LoopContext {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register calculator");
        LoopContext {
            conversation: ConversationContext {
                session_id: "session-batch".to_string(),
                message_id: "msg-batch".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: mode,
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        }
    }

    fn good_call() -> ToolCall {
        ToolCall { tool_id: "math.calculate".to_string(), input: json!({"expression": "1+1"}) }
    }

    fn bad_call() -> ToolCall {
        ToolCall { tool_id: "nonexistent.tool".to_string(), input: json!({}) }
    }

    #[tokio::test]
    async fn sequential_partial_stops_at_first_error() {
        let ctx = make_batch_context(ToolExecutionMode::SequentialPartial);
        let calls = vec![good_call(), bad_call(), good_call()];
        let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

        // Should have 2 results: the first success and the error (third call skipped)
        assert_eq!(journal.len(), 2);
        assert!(!journal[0].output.contains("ERROR"));
        assert!(journal[1].output.contains("ERROR"));
        assert!(result.contains("math.calculate"));
        assert!(result.contains("nonexistent.tool"));
        // The third good_call should NOT appear
        assert_eq!(result.matches("math.calculate").count(), 1);
    }

    #[tokio::test]
    async fn sequential_full_continues_past_errors() {
        let ctx = make_batch_context(ToolExecutionMode::SequentialFull);
        let calls = vec![good_call(), bad_call(), good_call()];
        let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

        // Should have all 3 results
        assert_eq!(journal.len(), 3);
        assert!(!journal[0].output.contains("ERROR"));
        assert!(journal[1].output.contains("ERROR"));
        assert!(!journal[2].output.contains("ERROR"));
        // math.calculate should appear twice
        assert_eq!(result.matches("math.calculate").count(), 2);
    }

    #[tokio::test]
    async fn parallel_executes_all_including_errors() {
        let ctx = make_batch_context(ToolExecutionMode::Parallel);
        let calls = vec![good_call(), bad_call(), good_call()];
        let (result, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

        // Should have all 3 results
        assert_eq!(journal.len(), 3);
        assert!(!journal[0].output.contains("ERROR"));
        assert!(journal[1].output.contains("ERROR"));
        assert!(!journal[2].output.contains("ERROR"));
        // math.calculate should appear twice
        assert_eq!(result.matches("math.calculate").count(), 2);
    }

    #[tokio::test]
    async fn sequential_partial_succeeds_when_no_errors() {
        let ctx = make_batch_context(ToolExecutionMode::SequentialPartial);
        let calls = vec![good_call(), good_call()];
        let (_, journal) = execute_tool_batch(&calls, &ctx, &[], None, None, None).await;

        assert_eq!(journal.len(), 2);
        assert!(journal.iter().all(|j| !j.output.contains("ERROR")));
    }

    /// A deny rule with bare email pattern `*@domain.com` must block
    /// `comm.send_external_message` to `user@domain.com` even without
    /// the `comm:` prefix in the scope.
    #[tokio::test]
    async fn bare_email_deny_rule_blocks_comm_send() {
        use hive_connectors::ConnectorServiceHandle;

        struct MockConnectorSvc;
        impl ConnectorServiceHandle for MockConnectorSvc {
            fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
                Some(DataClass::Public)
            }
            fn resolve_destination_approval(
                &self,
                _cid: &str,
                _dest: &str,
            ) -> Option<hive_contracts::ToolApproval> {
                None
            }
        }

        struct FakeSendTool(ToolDefinition);
        impl FakeSendTool {
            fn new() -> Self {
                Self(ToolDefinition {
                    id: "comm.send_external_message".to_string(),
                    name: "Send".to_string(),
                    description: "mock".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    channel_class: ChannelClass::Public,
                    side_effects: true,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: "Send".to_string(),
                        read_only_hint: None,
                        destructive_hint: None,
                        idempotent_hint: None,
                        open_world_hint: None,
                    },
                })
            }
        }
        impl Tool for FakeSendTool {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }
            fn execute(
                &self,
                _input: Value,
            ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        output: json!({"status": "sent"}),
                        data_class: DataClass::Public,
                    })
                })
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FakeSendTool::new())).unwrap();

        // Session permissions with a bare email deny rule (no `comm:` prefix)
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.send_external_message".to_string(),
            scope: "*@blocked.com".to_string(),
            decision: ToolApproval::Deny,
        });

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-deny-test".to_string(),
                message_id: "msg-deny-test".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(perms)),
                workspace_classification: None,
                effective_data_class: effective_dc,
                connector_service: Some(Arc::new(MockConnectorSvc)),
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
                Arc::new(MockConnectorSvc),
            )));
        let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

        // 1. Sending to user@blocked.com must be DENIED
        let result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "user@blocked.com",
                    "body": "hello"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err(), "send to user@blocked.com must be denied by bare email rule");
        let err = result.unwrap_err();
        assert!(matches!(err, LoopError::ToolDenied { .. }), "expected ToolDenied, got: {err:?}");

        // 2. Sending to user@allowed.com must SUCCEED (no matching deny rule)
        let result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "user@allowed.com",
                    "body": "hello"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_ok(), "send to user@allowed.com must not be blocked: {result:?}");
    }

    /// Deny rule with fully-qualified `comm:*:*@domain.com` scope must also work.
    #[tokio::test]
    async fn qualified_comm_deny_rule_blocks_comm_send() {
        use hive_connectors::ConnectorServiceHandle;

        struct MockConnectorSvc;
        impl ConnectorServiceHandle for MockConnectorSvc {
            fn resolve_output_class(&self, _cid: &str, _dest: &str) -> Option<DataClass> {
                Some(DataClass::Public)
            }
            fn resolve_destination_approval(
                &self,
                _cid: &str,
                _dest: &str,
            ) -> Option<hive_contracts::ToolApproval> {
                None
            }
        }

        struct FakeSendTool(ToolDefinition);
        impl FakeSendTool {
            fn new() -> Self {
                Self(ToolDefinition {
                    id: "comm.send_external_message".to_string(),
                    name: "Send".to_string(),
                    description: "mock".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    channel_class: ChannelClass::Public,
                    side_effects: true,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: "Send".to_string(),
                        read_only_hint: None,
                        destructive_hint: None,
                        idempotent_hint: None,
                        open_world_hint: None,
                    },
                })
            }
        }
        impl Tool for FakeSendTool {
            fn definition(&self) -> &ToolDefinition {
                &self.0
            }
            fn execute(
                &self,
                _input: Value,
            ) -> hive_tools::BoxFuture<'_, Result<ToolResult, hive_tools::ToolError>> {
                Box::pin(async {
                    Ok(ToolResult {
                        output: json!({"status": "sent"}),
                        data_class: DataClass::Public,
                    })
                })
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(FakeSendTool::new())).unwrap();

        // Fully-qualified deny rule
        let mut perms = SessionPermissions::new();
        perms.add_rule(PermissionRule {
            tool_pattern: "comm.*".to_string(),
            scope: "comm:*:*@blocked.com".to_string(),
            decision: ToolApproval::Deny,
        });

        let effective_dc = Arc::new(AtomicU8::new(DataClass::Public.to_i64() as u8));

        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-deny-qualified".to_string(),
                message_id: "msg-deny-qualified".to_string(),
                prompt: String::new(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Public,
                permissions: Arc::new(parking_lot::Mutex::new(perms)),
                workspace_classification: None,
                effective_data_class: effective_dc,
                connector_service: Some(Arc::new(MockConnectorSvc)),
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let classification_mw: Arc<dyn LoopMiddleware> =
            Arc::new(crate::classification_middleware::DataClassificationMiddleware::new(Some(
                Arc::new(MockConnectorSvc),
            )));
        let mw: &[Arc<dyn LoopMiddleware>] = &[classification_mw];

        let result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "comm.send_external_message".to_string(),
                input: json!({
                    "connector_id": "test-connector",
                    "to": "boss@blocked.com",
                    "body": "hello"
                }),
            },
            mw,
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err(), "qualified comm deny rule must block send");
        assert!(matches!(result.unwrap_err(), LoopError::ToolDenied { .. }));
    }

    // ── Preemption tests ────────────────────────────────────────────────

    #[test]
    fn build_preemption_summary_formats_tool_calls() {
        let mut journal = ConversationJournal::default();
        journal.record(JournalEntry {
            phase: JournalPhase::ToolCycle,
            turn: 1,
            tool_calls: vec![
                JournalToolCall {
                    tool_id: "file.read".to_string(),
                    input: r#"{"path":"src/main.rs"}"#.to_string(),
                    output: "fn main() {}".to_string(),
                },
                JournalToolCall {
                    tool_id: "search".to_string(),
                    input: r#"{"query":"auth"}"#.to_string(),
                    output: "3 results found".to_string(),
                },
            ],
        });

        let summary = super::build_preemption_summary(&journal);
        assert!(summary.contains("[Turn paused to process a new message]"));
        assert!(summary.contains("1. Called `file.read`"));
        assert!(summary.contains("2. Called `search`"));
        assert!(summary.contains("fn main() {}"));
    }

    #[test]
    fn build_preemption_summary_truncates_long_output() {
        let mut journal = ConversationJournal::default();
        let long_output = "x".repeat(300);
        journal.record(JournalEntry {
            phase: JournalPhase::ToolCycle,
            turn: 1,
            tool_calls: vec![JournalToolCall {
                tool_id: "file.read".to_string(),
                input: "{}".to_string(),
                output: long_output,
            }],
        });

        let summary = super::build_preemption_summary(&journal);
        // Output should be truncated to ~200 chars + ellipsis
        assert!(summary.contains("…"));
        assert!(summary.len() < 400);
    }

    #[test]
    fn build_preemption_summary_empty_journal() {
        let journal = ConversationJournal::default();
        let summary = super::build_preemption_summary(&journal);
        assert!(summary.contains("(no tool calls completed)"));
    }

    #[tokio::test]
    async fn check_preempt_returns_none_when_signal_not_set() {
        let signal = Arc::new(AtomicBool::new(false));
        let decision = RoutingDecision {
            selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
            fallback_chain: vec![],
            reason: String::new(),
        };

        let result = super::check_preempt(&Some(signal), &None, &decision, "p", "m", None).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn check_preempt_returns_none_when_no_signal() {
        let decision = RoutingDecision {
            selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
            fallback_chain: vec![],
            reason: String::new(),
        };

        let result = super::check_preempt(&None, &None, &decision, "p", "m", None).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn check_preempt_returns_result_when_signal_set() {
        let signal = Arc::new(AtomicBool::new(true));
        let decision = RoutingDecision {
            selected: ModelSelection { provider_id: "test".into(), model: "test-model".into() },
            fallback_chain: vec![],
            reason: String::new(),
        };

        let result =
            super::check_preempt(&Some(signal), &None, &decision, "test", "test-model", None).await;
        assert!(result.is_some());
        let r = result.unwrap();
        assert!(r.preempted);
        assert!(r.content.contains("[Turn paused"));
    }

    #[tokio::test]
    async fn check_preempt_emits_event() {
        let signal = Arc::new(AtomicBool::new(true));
        let decision = RoutingDecision {
            selected: ModelSelection { provider_id: "p".into(), model: "m".into() },
            fallback_chain: vec![],
            reason: String::new(),
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel::<LoopEvent>(10);

        let _ = super::check_preempt(&Some(signal), &None, &decision, "p", "m", Some(&tx)).await;

        let event = rx.try_recv().expect("should have emitted an event");
        assert!(matches!(event, LoopEvent::Preempted));
    }

    #[tokio::test]
    async fn react_preempts_after_tool_batch_when_signal_set() {
        // Set up: model returns a tool call, then (if not preempted) "done".
        // Signal is set before the loop starts, so it should preempt after
        // the first tool batch instead of calling the model a second time.
        let responses = vec![
            "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
                .to_string(),
            "done".to_string(),
        ];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register");
        let tools = Arc::new(registry);

        let signal = Arc::new(AtomicBool::new(true));

        let executor = LoopExecutor::new(Arc::new(ReActStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-preempt".to_string(),
                message_id: "msg-preempt".to_string(),
                prompt: "Calculate 1+1".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: Some(Arc::new(parking_lot::Mutex::new(
                    ConversationJournal::default(),
                ))),
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools,
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: Some(signal),
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");

        // Should have been preempted after the first tool batch
        assert!(result.preempted, "result should be preempted");
        assert!(result.content.contains("[Turn paused"));
        assert!(result.content.contains("math.calculate"));

        // Model should have been called only once (the tool-call response),
        // not a second time (the "done" response was never consumed).
        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 1, "model should only be called once before preemption");
    }

    #[tokio::test]
    async fn react_completes_normally_without_signal() {
        // Same setup as above but without preempt signal — should run to completion.
        let responses = vec![
            "<tool_call>{\"tool\":\"math.calculate\",\"input\":{\"expression\":\"1+1\"}}</tool_call>"
                .to_string(),
            "done".to_string(),
        ];
        let (provider, prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(CalculatorTool::default())).expect("register");

        let executor = LoopExecutor::new(Arc::new(ReActStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-normal".to_string(),
                message_id: "msg-normal".to_string(),
                prompt: "Calculate 1+1".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        assert!(!result.preempted, "should not be preempted");
        assert_eq!(result.content, "done");

        let recorded = prompts.lock().unwrap();
        assert_eq!(recorded.len(), 2, "model should be called twice");
    }

    #[tokio::test]
    async fn react_no_preempt_when_no_tools_called() {
        // Model returns plain text (no tool calls). Even with signal set,
        // there is no tool batch checkpoint, so it should complete normally.
        let responses = vec!["Just a text response".to_string()];
        let (provider, _prompts) = TestProvider::new(responses);
        let mut router = ModelRouter::new();
        router.register_provider(provider);
        let router = Arc::new(router);

        let signal = Arc::new(AtomicBool::new(true));

        let executor = LoopExecutor::new(Arc::new(ReActStrategy));
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "session-notools".to_string(),
                message_id: "msg-notools".to_string(),
                prompt: "Hello".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(ToolRegistry::new()),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: Vec::new(),
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: Some(signal),
            cancellation_token: None,
        };

        let result = executor.run(context, router).await.expect("loop result");
        // No tool batch → no checkpoint → no preemption
        assert!(!result.preempted);
        assert_eq!(result.content, "Just a text response");
    }

    // ---- strip_xml_tool_blocks tests ----

    #[test]
    fn test_strip_xml_tool_blocks_basic() {
        let input = r#"Hello<tool_call>{"tool":"core.ask_user","input":{}}</tool_call> world"#;
        assert_eq!(strip_xml_tool_blocks(input), "Hello world");
    }

    #[test]
    fn test_strip_xml_tool_blocks_multiple() {
        let input = "before<tool_call>first</tool_call>middle<tool_call>second</tool_call>after";
        assert_eq!(strip_xml_tool_blocks(input), "beforemiddleafter");
    }

    #[test]
    fn test_strip_xml_tool_blocks_with_result() {
        let input = "text<tool_call>{}</tool_call><tool_result>ok</tool_result>end";
        assert_eq!(strip_xml_tool_blocks(input), "textend");
    }

    #[test]
    fn test_strip_xml_tool_blocks_function_call() {
        let input = "hello<function_call>{}</function_call>world";
        assert_eq!(strip_xml_tool_blocks(input), "helloworld");
    }

    #[test]
    fn test_strip_xml_tool_blocks_no_tags() {
        assert_eq!(strip_xml_tool_blocks("plain text"), "plain text");
    }

    #[test]
    fn test_strip_xml_tool_blocks_only_tags() {
        let input = "<tool_call>stuff</tool_call>";
        assert_eq!(strip_xml_tool_blocks(input), "");
    }

    #[test]
    fn test_strip_xml_tool_blocks_unclosed() {
        let input = "hello<tool_call>stuff without close";
        assert_eq!(strip_xml_tool_blocks(input), "hello");
    }

    // ---- StreamingToolCallFilter tests ----

    #[test]
    fn test_streaming_filter_no_tags() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("hello "), "hello ");
        assert_eq!(f.feed("world"), "world");
        assert_eq!(f.flush(), "");
    }

    #[test]
    fn test_streaming_filter_complete_tag_single_delta() {
        let mut f = StreamingToolCallFilter::new();
        let out = f.feed("before<tool_call>payload</tool_call>after");
        assert_eq!(out, "beforeafter");
    }

    #[test]
    fn test_streaming_filter_tag_across_deltas() {
        let mut f = StreamingToolCallFilter::new();
        // Tag arrives split across multiple deltas
        assert_eq!(f.feed("hello "), "hello ");
        // Start of potential tag — buffered
        assert_eq!(f.feed("<tool"), "");
        // Completes the opening tag — suppress mode starts
        assert_eq!(f.feed("_call>payload"), "");
        // Close tag arrives — suppression ends
        assert_eq!(f.feed("</tool_call>done"), "done");
        assert_eq!(f.flush(), "");
    }

    #[test]
    fn test_streaming_filter_false_alarm() {
        let mut f = StreamingToolCallFilter::new();
        // Starts with < but doesn't match any tag
        assert_eq!(f.feed("x"), "x");
        // A < that turns out not to be a tag
        assert_eq!(f.feed("<"), ""); // buffered
        assert_eq!(f.feed("div>"), "<div>"); // flushed — not a tool tag
    }

    #[test]
    fn test_streaming_filter_flush_partial() {
        let mut f = StreamingToolCallFilter::new();
        assert_eq!(f.feed("<to"), ""); // could be <tool_call>
                                       // End of stream — flush whatever was buffered
        assert_eq!(f.flush(), "<to");
    }

    // ── Budget exemption tests ──────────────────────────────────────

    #[test]
    fn test_budget_exempt_agent_tools() {
        assert!(is_budget_exempt("core.list_agents"));
        assert!(is_budget_exempt("core.get_agent_result"));
        assert!(is_budget_exempt("core.wait_for_agent"));
    }

    #[test]
    fn test_budget_exempt_process_tools() {
        assert!(is_budget_exempt("process.status"));
        assert!(is_budget_exempt("process.list"));
    }

    #[test]
    fn test_budget_not_exempt_regular_tools() {
        assert!(!is_budget_exempt("core.spawn_agent"));
        assert!(!is_budget_exempt("core.signal_agent"));
        assert!(!is_budget_exempt("core.kill_agent"));
        assert!(!is_budget_exempt("shell.exec"));
        assert!(!is_budget_exempt("fs.read"));
        assert!(!is_budget_exempt("process.start"));
        assert!(!is_budget_exempt("process.kill"));
        assert!(!is_budget_exempt("process.write"));
    }

    // ── Access control tests ───────────────────────────────────────────────

    /// Orchestrator mock that tracks parent relationships for access control tests.
    struct AccessControlOrchestrator {
        /// Map of agent_id → parent_id.
        parents: std::collections::HashMap<String, Option<String>>,
        messages: Mutex<Vec<(String, String, String)>>,
        kills: Mutex<Vec<String>>,
    }

    impl AccessControlOrchestrator {
        fn new(parents: Vec<(&str, Option<&str>)>) -> Self {
            Self {
                parents: parents
                    .into_iter()
                    .map(|(id, parent)| (id.to_string(), parent.map(|s| s.to_string())))
                    .collect(),
                messages: Mutex::new(Vec::new()),
                kills: Mutex::new(Vec::new()),
            }
        }
    }

    impl AgentOrchestrator for AccessControlOrchestrator {
        fn spawn_agent(
            &self,
            _: Persona,
            _: String,
            _: Option<String>,
            _: Option<String>,
            _: hive_classification::DataClass,
            _: Option<hive_model::ModelSelection>,
            _: bool,
            _: Option<PathBuf>,
        ) -> BoxFuture<'_, Result<String, String>> {
            Box::pin(async { Ok("new-id".to_string()) })
        }
        fn message_agent(
            &self,
            agent_id: String,
            message: String,
            from: String,
        ) -> BoxFuture<'_, Result<(), String>> {
            self.messages.lock().unwrap().push((agent_id, message, from));
            Box::pin(async { Ok(()) })
        }
        fn message_session(&self, _: String, _: String) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn feedback_agent(
            &self,
            _: String,
            _: String,
            _: String,
        ) -> BoxFuture<'_, Result<(), String>> {
            Box::pin(async { Ok(()) })
        }
        fn list_agents(
            &self,
        ) -> BoxFuture<'_, Result<Vec<(String, String, String, String, Option<String>)>, String>>
        {
            Box::pin(async { Ok(vec![]) })
        }
        fn get_agent_result(
            &self,
            _: String,
        ) -> BoxFuture<'_, Result<(String, Option<String>), String>> {
            Box::pin(async { Ok(("Done".to_string(), None)) })
        }
        fn kill_agent(&self, agent_id: String) -> BoxFuture<'_, Result<(), String>> {
            self.kills.lock().unwrap().push(agent_id);
            Box::pin(async { Ok(()) })
        }
        fn get_agent_parent(
            &self,
            agent_id: String,
        ) -> BoxFuture<'_, Result<Option<String>, String>> {
            let parent = self.parents.get(&agent_id).cloned();
            Box::pin(async move { parent.ok_or_else(|| format!("agent '{agent_id}' not found")) })
        }
    }

    fn make_access_control_context(
        orchestrator: Arc<dyn AgentOrchestrator>,
        caller_id: Option<&str>,
        parent_id: Option<&str>,
    ) -> LoopContext {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(SignalAgentTool::default())).expect("register signal tool");
        registry.register(Arc::new(KillAgentTool::default())).expect("register kill tool");
        LoopContext {
            conversation: ConversationContext {
                session_id: "session-acl".to_string(),
                message_id: "msg-acl".to_string(),
                prompt: "test access".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: Some(Persona::default_persona()),
                agent_orchestrator: Some(orchestrator),
                personas: Vec::new(),
                current_agent_id: caller_id.map(|s| s.to_string()),
                parent_agent_id: parent_id.map(|s| s.to_string()),
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        }
    }

    #[tokio::test]
    async fn signal_agent_allows_parent_to_child() {
        // parent-agent spawned child-agent
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("parent-agent", None),
            ("child-agent", Some("parent-agent")),
        ]));
        let ctx = make_access_control_context(orch.clone(), Some("parent-agent"), None);
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "child-agent", "content": "hello"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn signal_agent_allows_child_to_parent() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("parent-agent", None),
            ("child-agent", Some("parent-agent")),
        ]));
        // child signals parent — caller_parent matches target_id
        let ctx =
            make_access_control_context(orch.clone(), Some("child-agent"), Some("parent-agent"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "parent-agent", "content": "done"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn signal_agent_allows_siblings() {
        // Both agents spawned by same parent
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("sibling-a", Some("root")),
            ("sibling-b", Some("root")),
        ]));
        let ctx = make_access_control_context(orch.clone(), Some("sibling-a"), Some("root"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "sibling-b", "content": "hey"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn signal_agent_allows_root_siblings() {
        // Both root-level agents (no parent — spawned by session)
        let orch =
            Arc::new(AccessControlOrchestrator::new(vec![("root-a", None), ("root-b", None)]));
        let ctx = make_access_control_context(orch.clone(), Some("root-a"), None);
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "root-b", "content": "hi"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn signal_agent_denies_unrelated_agents() {
        // agent-a and agent-x have different parents, not related
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("agent-a", Some("parent-1")),
            ("agent-x", Some("parent-2")),
        ]));
        let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("parent-1"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "agent-x", "content": "nope"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
    }

    #[tokio::test]
    async fn signal_agent_allows_bot_prefix() {
        // Bot targets always allowed for messaging, even without family relationship
        let orch = Arc::new(AccessControlOrchestrator::new(vec![]));
        let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("parent-1"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "bot:my-bot", "content": "work"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn signal_agent_allows_session_level_caller() {
        // Session-level callers (no current_agent_id) are always allowed
        let orch = Arc::new(AccessControlOrchestrator::new(vec![("any-agent", Some("someone"))]));
        let ctx = make_access_control_context(orch.clone(), None, None);
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.signal_agent".to_string(),
                input: json!({"agent_id": "any-agent", "content": "hi"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["delivered"], json!(true));
    }

    #[tokio::test]
    async fn kill_agent_allows_parent_to_kill_child() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("parent-agent", None),
            ("child-agent", Some("parent-agent")),
        ]));
        let ctx = make_access_control_context(orch.clone(), Some("parent-agent"), None);
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.kill_agent".to_string(),
                input: json!({"agent_id": "child-agent"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["killed"], json!(true));
        assert_eq!(orch.kills.lock().unwrap().as_slice(), ["child-agent"]);
    }

    #[tokio::test]
    async fn kill_agent_denies_child_killing_parent() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("parent-agent", None),
            ("child-agent", Some("parent-agent")),
        ]));
        let ctx =
            make_access_control_context(orch.clone(), Some("child-agent"), Some("parent-agent"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.kill_agent".to_string(),
                input: json!({"agent_id": "parent-agent"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
        assert!(orch.kills.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn kill_agent_denies_sibling_kill() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![
            ("sibling-a", Some("root")),
            ("sibling-b", Some("root")),
        ]));
        let ctx = make_access_control_context(orch.clone(), Some("sibling-a"), Some("root"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.kill_agent".to_string(),
                input: json!({"agent_id": "sibling-b"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
    }

    #[tokio::test]
    async fn kill_agent_denies_bot_prefix() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![]));
        let ctx = make_access_control_context(orch.clone(), Some("agent-a"), Some("root"));
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.kill_agent".to_string(),
                input: json!({"agent_id": "bot:my-bot"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert!(result.output["error"].as_str().unwrap().contains("Access denied"));
    }

    #[tokio::test]
    async fn kill_agent_allows_session_level_caller() {
        let orch = Arc::new(AccessControlOrchestrator::new(vec![("any-agent", Some("root"))]));
        let ctx = make_access_control_context(orch.clone(), None, None);
        let result = execute_tool_call(
            &ctx,
            ToolCall {
                tool_id: "core.kill_agent".to_string(),
                input: json!({"agent_id": "any-agent"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(result.output["killed"], json!(true));
    }

    #[tokio::test]
    async fn inject_pending_makes_question_visible() {
        let gate = UserInteractionGate::new();
        let kind = InteractionKind::Question {
            text: "What color?".into(),
            choices: vec!["red".into(), "blue".into()],
            allow_freeform: true,
            multi_select: false,
            message: None,
        };
        gate.inject_pending("q-1".to_string(), kind.clone());

        // Should appear in list_pending
        let pending = gate.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "q-1");

        // Responding should remove it from the gate
        let response = UserInteractionResponse {
            request_id: "q-1".to_string(),
            payload: InteractionResponsePayload::Answer {
                selected_choice: Some(0),
                selected_choices: None,
                text: None,
            },
        };
        assert!(gate.respond(response));
        assert!(gate.list_pending().is_empty());
    }

    #[tokio::test]
    async fn remove_all_except_clears_stale_injected_entries() {
        let gate = UserInteractionGate::new();
        let kind = InteractionKind::Question {
            text: "old question".into(),
            choices: vec![],
            allow_freeform: true,
            multi_select: false,
            message: None,
        };
        // Inject an old entry (simulating daemon restart injection)
        gate.inject_pending("old-q".to_string(), kind.clone());

        // Agent re-asks through the normal path → new entry
        let _rx = gate.create_request(
            "new-q".to_string(),
            InteractionKind::Question {
                text: "new question".into(),
                choices: vec![],
                allow_freeform: true,
                multi_select: false,
                message: None,
            },
        );

        assert_eq!(gate.list_pending().len(), 2);

        // Clean up stale entries, keeping the new one
        let removed = gate.remove_all_except("new-q");
        assert_eq!(removed, vec!["old-q"]);

        // Only the new entry should remain
        let pending = gate.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "new-q");
    }

    #[tokio::test]
    async fn close_gate_unblocks_pending_question() {
        let gate = Arc::new(UserInteractionGate::new());
        let rx = gate.create_request(
            "q-block".to_string(),
            InteractionKind::Question {
                text: "blocking question".into(),
                choices: vec![],
                allow_freeform: true,
                multi_select: false,
                message: None,
            },
        );

        // Spawn a task that waits on the receiver
        let gate_clone = Arc::clone(&gate);
        let handle = tokio::spawn(async move {
            // Close the gate after a short delay to unblock the receiver
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            gate_clone.close();
        });

        // rx.await should resolve with Err(RecvError) once the gate is closed
        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
        assert!(result.is_ok(), "should not timeout — gate.close() should unblock");
        assert!(result.unwrap().is_err(), "should receive RecvError when sender is dropped");

        // Gate should be empty
        assert!(gate.list_pending().is_empty());
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn close_gate_unblocks_pending_approval() {
        let gate = Arc::new(UserInteractionGate::new());
        let rx = gate.create_request(
            "approve-block".to_string(),
            InteractionKind::ToolApproval {
                tool_id: "shell.execute".into(),
                input: r#"{"command": "ls"}"#.into(),
                reason: "needs approval".into(),
                inferred_scope: None,
            },
        );

        let gate_clone = Arc::clone(&gate);
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            gate_clone.close();
        });

        let result = tokio::time::timeout(std::time::Duration::from_secs(2), rx).await;
        assert!(result.is_ok(), "should not timeout");
        assert!(result.unwrap().is_err(), "should receive RecvError");
        assert!(gate.list_pending().is_empty());
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn execute_tool_call_cancelled_by_token() {
        // A tool that sleeps forever — cancellation should interrupt it
        struct SlowTool {
            def: ToolDefinition,
        }
        impl Tool for SlowTool {
            fn definition(&self) -> &ToolDefinition {
                &self.def
            }
            fn execute(
                &self,
                _input: serde_json::Value,
            ) -> hive_tools::BoxFuture<'_, Result<hive_tools::ToolResult, hive_tools::ToolError>>
            {
                Box::pin(async {
                    // Sleep indefinitely — only cancellation can stop us
                    tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
                    Ok(hive_tools::ToolResult {
                        output: json!({"done": true}),
                        data_class: DataClass::Internal,
                    })
                })
            }
        }

        let mut registry = ToolRegistry::new();
        registry
            .register(Arc::new(SlowTool {
                def: ToolDefinition {
                    id: "test.slow".to_string(),
                    name: "slow_tool".to_string(),
                    description: "a slow tool".to_string(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    channel_class: ChannelClass::Internal,
                    side_effects: false,
                    approval: ToolApproval::Auto,
                    annotations: ToolAnnotations {
                        title: "slow".to_string(),
                        read_only_hint: Some(true),
                        destructive_hint: Some(false),
                        idempotent_hint: Some(true),
                        open_world_hint: Some(false),
                    },
                },
            }))
            .unwrap();

        let token = tokio_util::sync::CancellationToken::new();
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "cancel-test".to_string(),
                message_id: "msg-cancel".to_string(),
                prompt: "test".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new(registry),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: vec![],
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: Some(token.clone()),
        };

        // Cancel after a short delay
        let token_clone = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            token_clone.cancel();
        });

        let result = execute_tool_call(
            &context,
            ToolCall { tool_id: "test.slow".to_string(), input: json!({}) },
            &[],
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            LoopError::Cancelled => {} // expected
            other => panic!("expected LoopError::Cancelled, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn execute_tool_call_without_token_runs_normally() {
        // Verify tool execution works normally when no cancellation token is set
        let context = LoopContext {
            conversation: ConversationContext {
                session_id: "no-cancel-test".to_string(),
                message_id: "msg-no-cancel".to_string(),
                prompt: "test".to_string(),
                prompt_content_parts: vec![],
                history: vec![],
                conversation_journal: None,
                initial_tool_iterations: 0,
            },
            routing: RoutingConfig {
                required_capabilities: BTreeSet::new(),
                preferred_models: None,
                loop_strategy: None,
                routing_decision: None,
            },
            security: SecurityContext {
                data_class: DataClass::Internal,
                permissions: Arc::new(parking_lot::Mutex::new(SessionPermissions::new())),
                workspace_classification: None,
                effective_data_class: Arc::new(AtomicU8::new(DataClass::Internal.to_i64() as u8)),
                connector_service: None,
                shadow_mode: false,
            },
            tools_ctx: ToolsContext {
                tools: Arc::new({
                    let mut r = ToolRegistry::new();
                    r.register(Arc::new(CalculatorTool::default())).unwrap();
                    r
                }),
                skill_catalog: None,
                knowledge_query_handler: None,
                tool_execution_mode: ToolExecutionMode::default(),
            },
            agent: AgentContext {
                persona: None,
                agent_orchestrator: None,
                personas: vec![],
                current_agent_id: None,
                parent_agent_id: None,
                workspace_path: None,
                keep_alive: false,
                session_messaged: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            },
            tool_limits: ToolLimitsConfig::default(),
            preempt_signal: None,
            cancellation_token: None,
        };

        let result = execute_tool_call(
            &context,
            ToolCall {
                tool_id: "math.calculate".to_string(),
                input: json!({"expression": "2 + 2"}),
            },
            &[],
            None,
            None,
            None,
        )
        .await;

        assert!(result.is_ok(), "tool should execute normally without cancellation token");
    }
}
