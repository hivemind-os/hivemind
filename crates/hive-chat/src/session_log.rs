//! Per-session file logging.
//!
//! Creates `<hivemind_home>/sessions/<session_id>/` with log files:
//! - `chat.log`   – main chat thread events (user messages, assistant responses)
//! - `loop.log`   – agent loop events (tool calls, model loading, routing)
//! - `agent-<id>.log` – one file per sub-agent

use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::Local;
use hive_loop::LoopEvent;

use crate::SessionEvent;
use hive_agents::SupervisorEvent;
use hive_contracts::ReasoningEvent;

/// File-based logger for a single chat session.
///
/// Log files are opened lazily on first write and closed immediately after,
/// so idle sessions hold **zero** file descriptors. This is critical because
/// the daemon may have hundreds of restored sessions.
pub struct SessionLogger {
    session_dir: PathBuf,
}

impl SessionLogger {
    /// Prepare a logger for the given session. Only creates the directory;
    /// log files are opened lazily on write.
    pub fn new(sessions_root: &Path, session_id: &str) -> std::io::Result<Self> {
        let session_dir = sessions_root.join(session_id);
        fs::create_dir_all(&session_dir)?;
        Ok(Self { session_dir })
    }

    /// Log a chat-level event (user message sent, assistant response, errors).
    pub fn log_chat(&self, line: &str) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        if let Ok(mut f) = open_append(&self.session_dir.join("chat.log")) {
            let _ = writeln!(f, "[{ts}] {line}");
        }
    }

    /// Log an agent-loop event (tool calls, model routing, tokens).
    pub fn log_loop(&self, line: &str) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        if let Ok(mut f) = open_append(&self.session_dir.join("loop.log")) {
            let _ = writeln!(f, "[{ts}] {line}");
        }
    }

    /// Log a sub-agent event to its dedicated file.
    pub fn log_agent(&self, agent_id: &str, line: &str) {
        let ts = Local::now().format("%H:%M:%S%.3f");
        let path = self.session_dir.join(format!("agent-{agent_id}.log"));
        if let Ok(mut f) = open_append(&path) {
            let _ = writeln!(f, "[{ts}] {line}");
        }
    }

    /// Route a `SessionEvent` to the appropriate log file(s).
    pub fn handle_event(&self, event: &SessionEvent) {
        match event {
            SessionEvent::Loop(loop_event) => self.handle_loop_event(loop_event),
            SessionEvent::Supervisor(sup_event) => self.handle_supervisor_event(sup_event),
        }
    }

    fn handle_loop_event(&self, event: &LoopEvent) {
        match event {
            LoopEvent::Token { .. } => {
                // Don't log individual tokens — too noisy
            }
            LoopEvent::ModelLoading { provider_id, model, .. } => {
                self.log_loop(&format!("MODEL_LOADING provider={provider_id} model={model}"));
            }
            LoopEvent::ModelDone { content, provider_id, model } => {
                let preview = truncate(content, 200);
                self.log_loop(&format!(
                    "MODEL_DONE provider={provider_id} model={model} content={preview}"
                ));
            }
            LoopEvent::ToolCallStart { tool_id, input } => {
                let preview = truncate(input, 300);
                self.log_loop(&format!("TOOL_START {tool_id} input={preview}"));
            }
            LoopEvent::ToolCallResult { tool_id, output, is_error } => {
                let preview = truncate(output, 300);
                let tag = if *is_error { "TOOL_ERROR" } else { "TOOL_RESULT" };
                self.log_loop(&format!("{tag} {tool_id} output={preview}"));
            }
            LoopEvent::Done { content, provider_id, model } => {
                let preview = truncate(content, 200);
                self.log_loop(&format!(
                    "LOOP_DONE provider={provider_id} model={model} content={preview}"
                ));
                self.log_chat(&format!("ASSISTANT [{provider_id}/{model}] {preview}"));
            }
            LoopEvent::UserInteractionRequired { request_id, kind, .. } => {
                self.log_loop(&format!("APPROVAL_REQUIRED id={request_id} kind={kind:?}"));
            }
            LoopEvent::Error { message, error_code, http_status, .. } => {
                let code_str = error_code.as_deref().unwrap_or("unknown");
                let status_str = http_status.map(|s| format!(" http={s}")).unwrap_or_default();
                self.log_loop(&format!("ERROR [{code_str}{status_str}] {message}"));
                self.log_chat(&format!("ERROR [{code_str}{status_str}] {message}"));
            }
            LoopEvent::ModelRetry {
                provider_id,
                model,
                attempt,
                max_attempts,
                error_kind,
                http_status,
                backoff_ms,
            } => {
                let status_str = http_status.map(|s| format!(" http={s}")).unwrap_or_default();
                self.log_loop(&format!(
                    "MODEL_RETRY provider={provider_id} model={model} attempt={attempt}/{max_attempts} kind={error_kind}{status_str} backoff={backoff_ms}ms"
                ));
            }
            LoopEvent::AgentSessionMessage { from_agent_id, content } => {
                let preview = truncate(content, 200);
                self.log_chat(&format!("AGENT_MESSAGE from={from_agent_id} {preview}"));
            }
            LoopEvent::ModelFallback { from_provider, from_model, to_provider, to_model } => {
                self.log_loop(&format!(
                    "MODEL_FALLBACK from={from_provider}:{from_model} to={to_provider}:{to_model}"
                ));
            }
            LoopEvent::BudgetExtended { new_budget, extensions_granted } => {
                self.log_loop(&format!(
                    "BUDGET_EXTENDED new_budget={new_budget} extensions={extensions_granted}"
                ));
            }
            LoopEvent::StallWarning { tool_name, repeated_count } => {
                self.log_loop(&format!("STALL_WARNING tool={tool_name} repeated={repeated_count}"));
            }
            LoopEvent::Preempted => {
                self.log_loop("PREEMPTED turn yielded for new user message");
            }
            LoopEvent::ToolCallArgDelta { .. } => {
                // High-frequency streaming event — not logged.
            }
            LoopEvent::ToolCallIntercepted { tool_id, input } => {
                let preview = truncate(input, 300);
                self.log_loop(&format!("TOOL_INTERCEPTED {tool_id} input={preview}"));
            }
            LoopEvent::CodeExecution { code, stdout, stderr, is_error, .. } => {
                let code_preview = truncate(code, 100);
                let output_preview = if !stdout.is_empty() {
                    truncate(stdout, 200)
                } else {
                    truncate(stderr, 200)
                };
                let tag = if *is_error { "CODE_ERROR" } else { "CODE_EXEC" };
                self.log_loop(&format!("{tag} code={code_preview} output={output_preview}"));
            }
        }
    }

    fn handle_supervisor_event(&self, event: &SupervisorEvent) {
        match event {
            SupervisorEvent::AgentSpawned { agent_id, spec, .. } => {
                self.log_chat(&format!(
                    "AGENT_SPAWNED id={agent_id} name={} keep_alive={}",
                    spec.friendly_name, spec.keep_alive,
                ));
                self.log_agent(
                    agent_id,
                    &format!("SPAWNED name={} role={:?}", spec.friendly_name, spec.role),
                );
            }
            SupervisorEvent::AgentStatusChanged { agent_id, status } => {
                self.log_agent(agent_id, &format!("STATUS {status:?}"));
            }
            SupervisorEvent::AgentCompleted { agent_id, result } => {
                let preview = truncate(result, 200);
                self.log_agent(agent_id, &format!("COMPLETED {preview}"));
                self.log_chat(&format!("AGENT_COMPLETED id={agent_id} {preview}"));
            }
            SupervisorEvent::AgentOutput { agent_id, event } => {
                let line = format_reasoning_event(event);
                self.log_agent(agent_id, &line);
            }
            SupervisorEvent::AgentTaskAssigned { agent_id, task } => {
                let preview = truncate(task, 200);
                self.log_agent(agent_id, &format!("TASK_ASSIGNED {preview}"));
            }
            SupervisorEvent::MessageRouted { from, to, msg_type } => {
                self.log_loop(&format!("MSG_ROUTED from={from} to={to} type={msg_type}"));
            }
            SupervisorEvent::AllComplete { total_messages } => {
                self.log_loop(&format!("ALL_AGENTS_COMPLETE total_messages={total_messages}"));
            }
        }
    }

    // ── JSONL event persistence ─────────────────────────────────────

    /// Persist a `SupervisorEvent` as a JSON line in `agent-{id}.events.jsonl`.
    pub fn persist_event(&self, event: &SupervisorEvent) {
        let agent_id = match event {
            SupervisorEvent::AgentSpawned { agent_id, .. }
            | SupervisorEvent::AgentStatusChanged { agent_id, .. }
            | SupervisorEvent::AgentTaskAssigned { agent_id, .. }
            | SupervisorEvent::AgentOutput { agent_id, .. }
            | SupervisorEvent::AgentCompleted { agent_id, .. } => agent_id.as_str(),
            SupervisorEvent::MessageRouted { .. } | SupervisorEvent::AllComplete { .. } => return,
        };
        let Ok(line) = serde_json::to_string(event) else { return };
        let path = self.session_dir.join(format!("agent-{agent_id}.events.jsonl"));
        if let Ok(mut f) = open_append(&path) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Read persisted events for an agent from its JSONL file.
    /// Returns `(events_page, total_count)` with oldest-first ordering.
    pub fn read_agent_events_paged(
        &self,
        agent_id: &str,
        offset: usize,
        limit: usize,
    ) -> (Vec<SupervisorEvent>, usize) {
        let path = self.session_dir.join(format!("agent-{agent_id}.events.jsonl"));
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return (Vec::new(), 0),
        };
        let reader = BufReader::new(file);
        let all_events: Vec<SupervisorEvent> = reader
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect();
        let total = all_events.len();
        let start = offset.min(total);
        let end = (start + limit).min(total);
        (all_events[start..end].to_vec(), total)
    }

    /// List agent IDs that have persisted JSONL event files.
    pub fn list_persisted_agent_ids(&self) -> Vec<String> {
        let Ok(entries) = fs::read_dir(&self.session_dir) else {
            return Vec::new();
        };
        entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with("agent-") && name.ends_with(".events.jsonl") {
                    Some(
                        name.trim_start_matches("agent-")
                            .trim_end_matches(".events.jsonl")
                            .to_string(),
                    )
                } else {
                    None
                }
            })
            .collect()
    }

    // ── Session-level event persistence ─────────────────────────────

    /// Persist a `SessionEvent` to the session-level `events.jsonl`.
    /// Skips token deltas (too noisy).
    pub fn persist_session_event(&self, event: &SessionEvent) {
        // Skip noisy token events
        if matches!(event, SessionEvent::Loop(LoopEvent::Token { .. })) {
            return;
        }
        // Skip token deltas nested in supervisor output
        if matches!(
            event,
            SessionEvent::Supervisor(SupervisorEvent::AgentOutput {
                event: ReasoningEvent::TokenDelta { .. },
                ..
            })
        ) {
            return;
        }
        let Ok(line) = serde_json::to_string(event) else { return };
        if let Ok(mut f) = open_append(&self.session_dir.join("events.jsonl")) {
            let _ = writeln!(f, "{line}");
        }
    }

    /// Read session-level events from `events.jsonl`.
    /// Returns `(events_page, total_count)` with oldest-first ordering.
    pub fn read_session_events_paged(
        &self,
        offset: usize,
        limit: usize,
    ) -> (Vec<SessionEvent>, usize) {
        let path = self.session_dir.join("events.jsonl");
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(_) => return (Vec::new(), 0),
        };
        let reader = BufReader::new(file);
        let all_events: Vec<SessionEvent> = reader
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| serde_json::from_str(&line).ok())
            .collect();
        let total = all_events.len();
        let start = offset.min(total);
        let end = (start + limit).min(total);
        (all_events[start..end].to_vec(), total)
    }
}

fn format_reasoning_event(event: &ReasoningEvent) -> String {
    match event {
        ReasoningEvent::StepStarted { step_id, description } => {
            format!("STEP_STARTED {step_id} {}", truncate(description, 200))
        }
        ReasoningEvent::ModelCallStarted { model, prompt_preview, .. } => {
            format!("MODEL_START model={model} {}", truncate(prompt_preview, 200))
        }
        ReasoningEvent::ModelCallCompleted { content, token_count, .. } => {
            format!("MODEL_DONE tokens={token_count} {}", truncate(content, 200))
        }
        ReasoningEvent::ToolCallStarted { tool_id, input } => {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            format!("TOOL_START {} input={}", tool_id, truncate(&input_str, 300))
        }
        ReasoningEvent::ToolCallCompleted { tool_id, output, is_error } => {
            let output_str = serde_json::to_string(output).unwrap_or_default();
            let tag = if *is_error { "TOOL_ERROR" } else { "TOOL_RESULT" };
            format!("{tag} {} output={}", tool_id, truncate(&output_str, 300))
        }
        ReasoningEvent::BranchEvaluated { condition, result } => {
            format!("BRANCH condition={condition} result={result}")
        }
        ReasoningEvent::PathAbandoned { reason } => {
            format!("PATH_ABANDONED {reason}")
        }
        ReasoningEvent::Synthesized { sources, result } => {
            format!("SYNTHESIZED sources={} {}", sources.len(), truncate(result, 200))
        }
        ReasoningEvent::Completed { result } => {
            format!("COMPLETED {}", truncate(result, 200))
        }
        ReasoningEvent::Failed { error, error_code, http_status, .. } => {
            let code_str = error_code.as_deref().unwrap_or("unknown");
            let status_str = http_status.map(|s| format!(" http={s}")).unwrap_or_default();
            format!("FAILED [{code_str}{status_str}] {error}")
        }
        ReasoningEvent::TokenDelta { .. } => {
            // Don't log individual token deltas
            String::new()
        }
        ReasoningEvent::UserInteractionRequired { request_id, tool_id, reason, .. } => {
            format!("APPROVAL_REQUIRED id={request_id} tool={tool_id} reason={reason}")
        }
        ReasoningEvent::QuestionAsked { request_id, agent_id, text, .. } => {
            format!("QUESTION id={request_id} agent={agent_id} text={}", truncate(text, 200))
        }
        ReasoningEvent::ModelRetry {
            provider_id,
            model,
            attempt,
            max_attempts,
            error_kind,
            http_status,
            backoff_ms,
        } => {
            let status_str = http_status.map(|s| format!(" http={s}")).unwrap_or_default();
            format!(
                "MODEL_RETRY provider={provider_id} model={model} attempt={attempt}/{max_attempts} kind={error_kind}{status_str} backoff={backoff_ms}ms"
            )
        }
        ReasoningEvent::ToolCallArgDelta { .. } => {
            // High-frequency streaming event — not logged to session summary.
            return String::new();
        }
        ReasoningEvent::ToolCallIntercepted { tool_id, input } => {
            let input_str = serde_json::to_string(input).unwrap_or_default();
            format!("TOOL_INTERCEPTED {} input={}", tool_id, truncate(&input_str, 300))
        }
        ReasoningEvent::CodeExecution { code, stdout, stderr, is_error, .. } => {
            let code_preview = truncate(code, 100);
            let output_preview = if !stdout.is_empty() {
                truncate(stdout, 200)
            } else {
                truncate(stderr, 200)
            };
            if *is_error {
                format!("CODE_ERROR code={} output={}", code_preview, output_preview)
            } else {
                format!("CODE_EXEC code={} output={}", code_preview, output_preview)
            }
        }
    }
}
fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.replace('\n', "\\n")
    } else {
        // Walk back from `max` to find a valid char boundary to avoid
        // slicing inside a multi-byte UTF-8 character.
        let mut end = max;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", s[..end].replace('\n', "\\n"))
    }
}

fn open_append(path: &Path) -> std::io::Result<File> {
    OpenOptions::new().create(true).append(true).open(path)
}
