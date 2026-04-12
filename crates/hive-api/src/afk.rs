//! AFK forwarding service — background task that watches for pending
//! interactions and forwards them to configured communication channels
//! when the user's status is non-active, and routes inbound responses
//! (button clicks, email replies) back to resolve interaction gates.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use tokio::sync::broadcast;
use tracing::{debug, info, trace, warn, Instrument};

use hive_chat::{ApprovalStreamEvent, ChatService};
use hive_connectors::services::interaction_format::{
    self, FormattedInteraction, ForwardedInteractionInfo,
};
use hive_connectors::ConnectorService;
use hive_contracts::{InteractionResponsePayload, UserInteractionResponse};
use hive_core::{EventBus, HiveMindConfig};

use crate::UserStatusRuntime;

// ---------------------------------------------------------------------------
// Forwarded interaction tracking
// ---------------------------------------------------------------------------

/// Tracks a forwarded interaction so we can update/delete the channel message
/// when the interaction is resolved locally.
#[derive(Debug, Clone)]
pub struct ForwardedInteraction {
    pub request_id: String,
    pub connector_id: String,
    /// Provider-specific channel (Slack channel ID, Discord channel ID).
    pub channel_id: String,
    /// Provider-specific message identifier (Slack ts, Discord message ID, email Message-ID).
    pub channel_message_id: String,
    /// Thread identifier for matching threaded replies (Slack thread_ts, Discord message_id).
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub agent_id: String,
    pub agent_name: String,
    pub workflow_name: Option<String>,
    /// The kind of interaction (approval or question) — needed for response routing.
    pub kind: ForwardedKind,
    /// When the interaction was forwarded — used for auto-approve timeout.
    pub forwarded_at: std::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardedKind {
    Approval,
    Question,
}

/// Shared store of currently-forwarded interactions.
pub type ForwardedStore = Arc<parking_lot::Mutex<HashMap<String, ForwardedInteraction>>>;

// ---------------------------------------------------------------------------
// HTML → plain-text helpers
// ---------------------------------------------------------------------------

/// Lightweight HTML-to-text conversion for inbound email replies.
///
/// Handles common elements: `<br>`, `<p>`, `<div>`, `<hr>`, block tags produce
/// line breaks, `<style>`/`<script>` blocks are stripped entirely, and HTML
/// entities (`&amp;`, `&lt;`, `&gt;`, `&quot;`, `&#…;`) are decoded.
fn strip_html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut chars = html.chars().peekable();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut skip_content = false; // inside <style> or <script>

    while let Some(ch) = chars.next() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tag = tag_buf.trim().to_lowercase();
                let tag_name =
                    tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");

                // Toggle content-skip for <style>/<script>.
                if tag_name == "style" || tag_name == "script" {
                    skip_content = true;
                }
                if tag.starts_with("/style") || tag.starts_with("/script") {
                    skip_content = false;
                    continue;
                }

                if !skip_content {
                    // Block-level elements → newline.
                    match tag_name {
                        "br" | "p" | "div" | "tr" | "li" | "hr" | "h1" | "h2" | "h3" | "h4"
                        | "h5" | "h6" | "blockquote" => {
                            out.push('\n');
                        }
                        _ => {}
                    }
                    // Closing block tags also get a newline.
                    if tag.starts_with('/') {
                        let closing = &tag_name;
                        if matches!(
                            *closing,
                            "" | "p"
                                | "div"
                                | "tr"
                                | "li"
                                | "h1"
                                | "h2"
                                | "h3"
                                | "h4"
                                | "h5"
                                | "h6"
                                | "blockquote"
                        ) {
                            out.push('\n');
                        }
                    }
                }
            } else {
                tag_buf.push(ch);
            }
            continue;
        }
        if skip_content {
            continue;
        }

        // Decode HTML entities.
        if ch == '&' {
            let mut entity = String::new();
            for ec in chars.by_ref() {
                if ec == ';' {
                    break;
                }
                entity.push(ec);
                if entity.len() > 10 {
                    break;
                }
            }
            match entity.as_str() {
                "amp" => out.push('&'),
                "lt" => out.push('<'),
                "gt" => out.push('>'),
                "quot" => out.push('"'),
                "apos" => out.push('\''),
                "nbsp" => out.push(' '),
                _ if entity.starts_with('#') => {
                    let num = if entity.starts_with("#x") || entity.starts_with("#X") {
                        u32::from_str_radix(&entity[2..], 16).ok()
                    } else {
                        entity[1..].parse::<u32>().ok()
                    };
                    if let Some(c) = num.and_then(char::from_u32) {
                        out.push(c);
                    }
                }
                _ => {
                    out.push('&');
                    out.push_str(&entity);
                    out.push(';');
                }
            }
            continue;
        }
        out.push(ch);
    }

    // Collapse runs of blank lines into at most two newlines.
    let mut result = String::with_capacity(out.len());
    let mut consecutive_newlines = 0u32;
    for ch in out.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                result.push('\n');
            }
        } else {
            consecutive_newlines = 0;
            result.push(ch);
        }
    }
    result
}

/// Returns true if the body looks like HTML content.
fn looks_like_html(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with('<')
        && (trimmed.len() > 5)
        && (trimmed[..trimmed.len().min(200)].to_lowercase().contains("<html")
            || trimmed[..trimmed.len().min(200)].to_lowercase().contains("<div")
            || trimmed[..trimmed.len().min(200)].to_lowercase().contains("<p")
            || trimmed[..trimmed.len().min(200)].to_lowercase().contains("<!doctype"))
}

// ---------------------------------------------------------------------------
// Email reply parsing
// ---------------------------------------------------------------------------

/// Parsed response from an email reply body.
#[derive(Debug)]
enum ParsedEmailResponse {
    Approve,
    Deny,
    ChoiceIndex(usize),
    FreeformText(String),
}

/// Parse the first meaningful line of an email reply for approve/deny/choice.
fn parse_email_reply_body(body: &str) -> Option<ParsedEmailResponse> {
    // Strip quoted lines (starting with '>') and common signature markers.
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Stop at quoted text or signature
        if trimmed.starts_with('>') || trimmed == "--" || trimmed == "---" {
            break;
        }
        let lower = trimmed.to_lowercase();
        if lower == "approve" || lower == "approved" || lower == "yes" {
            return Some(ParsedEmailResponse::Approve);
        }
        if lower == "deny" || lower == "denied" || lower == "no" || lower == "reject" {
            return Some(ParsedEmailResponse::Deny);
        }
        // Try parsing as a choice number (1-indexed)
        if let Ok(n) = trimmed.parse::<usize>() {
            if n > 0 {
                return Some(ParsedEmailResponse::ChoiceIndex(n - 1));
            }
        }
        // Otherwise treat as free-form text answer
        return Some(ParsedEmailResponse::FreeformText(trimmed.to_string()));
    }
    None
}

// ---------------------------------------------------------------------------
// Inbound response matching
// ---------------------------------------------------------------------------

/// Parsed inbound interaction response from any channel.
struct InboundResponse {
    request_id: String,
    payload: InteractionResponsePayload,
}

/// Try to extract an interaction response from an inbound message's metadata.
///
/// Handles:
/// - Slack block_actions (interaction_action_id = "hivemind_approve:<id>" etc.)
/// - Discord button clicks (interaction_custom_id = "hivemind_approve:<id>" etc.)
/// - Threaded text replies (Slack thread_ts, Discord reply reference matching)
/// - Email replies (hivemind_token or in_reply_to matching)
fn try_match_inbound(
    metadata: &HashMap<String, String>,
    body: &str,
    store: &ForwardedStore,
) -> Option<InboundResponse> {
    // 1. Slack block_actions
    if let Some(action_id) = metadata.get("interaction_action_id") {
        return parse_hivemind_action(action_id);
    }

    // 2. Discord button clicks
    if let Some(custom_id) = metadata.get("interaction_custom_id") {
        return parse_hivemind_action(custom_id);
    }

    // 3. Slack threaded reply — match thread_ts to a forwarded message's ts
    if let Some(thread_ts) = metadata.get("thread_ts") {
        // Only if this isn't a button click (no interaction metadata)
        if metadata.get("interaction_action_id").is_none() {
            let store_lock = store.lock();
            let entry = store_lock.values().find(|fi| fi.thread_id.as_deref() == Some(thread_ts));
            if let Some(fi) = entry {
                let request_id = fi.request_id.clone();
                let kind = fi.kind;
                drop(store_lock);
                return build_text_response(&request_id, kind, body);
            }
        }
    }

    // 4. Discord reply reference — match message_id of the referenced message
    //    (Discord includes the referenced message_id in the reply's metadata)
    if let Some(ref_msg_id) = metadata.get("referenced_message_id") {
        let store_lock = store.lock();
        let entry = store_lock.values().find(|fi| fi.channel_message_id == *ref_msg_id);
        if let Some(fi) = entry {
            let request_id = fi.request_id.clone();
            let kind = fi.kind;
            drop(store_lock);
            return build_text_response(&request_id, kind, body);
        }
    }

    // 5. Email: match by hivemind_token in subject
    if let Some(token) = metadata.get("hivemind_token") {
        return match_email_reply(token, body, store);
    }

    // 6. Email: match by In-Reply-To header → stored channel_message_id
    if let Some(in_reply_to) = metadata.get("in_reply_to") {
        let store_lock = store.lock();
        let entry = store_lock.values().find(|fi| fi.channel_message_id == *in_reply_to);
        if let Some(fi) = entry {
            let request_id = fi.request_id.clone();
            let kind = fi.kind;
            drop(store_lock);
            return build_email_response(&request_id, kind, body);
        }
    }

    None
}

/// Parse a hivemind action ID like "hivemind_approve:<request_id>" or "hivemind_choice:<id>:<idx>"
fn parse_hivemind_action(action_id: &str) -> Option<InboundResponse> {
    if let Some(request_id) = action_id.strip_prefix("hivemind_approve:") {
        return Some(InboundResponse {
            request_id: request_id.to_string(),
            payload: InteractionResponsePayload::ToolApproval {
                approved: true,
                allow_session: false,
                allow_agent: false,
            },
        });
    }
    if let Some(request_id) = action_id.strip_prefix("hivemind_deny:") {
        return Some(InboundResponse {
            request_id: request_id.to_string(),
            payload: InteractionResponsePayload::ToolApproval {
                approved: false,
                allow_session: false,
                allow_agent: false,
            },
        });
    }
    if let Some(rest) = action_id.strip_prefix("hivemind_choice:") {
        // "hivemind_choice:<request_id>:<index>"
        let parts: Vec<&str> = rest.splitn(2, ':').collect();
        if parts.len() == 2 {
            if let Ok(idx) = parts[1].parse::<usize>() {
                return Some(InboundResponse {
                    request_id: parts[0].to_string(),
                    payload: InteractionResponsePayload::Answer {
                        selected_choice: Some(idx),
                        selected_choices: None,
                        text: None,
                    },
                });
            }
        }
    }
    None
}

/// Build a response from an email reply body matched to a forwarded interaction.
fn match_email_reply(token: &str, body: &str, store: &ForwardedStore) -> Option<InboundResponse> {
    let store_lock = store.lock();
    // The token is a short prefix of the request_id
    let entry = store_lock.values().find(|fi| fi.request_id.starts_with(token));
    let (request_id, kind) = entry.map(|fi| (fi.request_id.clone(), fi.kind))?;
    drop(store_lock);
    build_email_response(&request_id, kind, body)
}

fn build_email_response(
    request_id: &str,
    kind: ForwardedKind,
    body: &str,
) -> Option<InboundResponse> {
    // Email replies from HTML messages arrive as HTML; convert to plain text.
    let text_body;
    let effective_body = if looks_like_html(body) {
        text_body = strip_html_to_text(body);
        text_body.as_str()
    } else {
        body
    };
    let parsed = parse_email_reply_body(effective_body)?;
    let payload = match kind {
        ForwardedKind::Approval => match parsed {
            ParsedEmailResponse::Approve => InteractionResponsePayload::ToolApproval {
                approved: true,
                allow_session: false,
                allow_agent: false,
            },
            ParsedEmailResponse::Deny => InteractionResponsePayload::ToolApproval {
                approved: false,
                allow_session: false,
                allow_agent: false,
            },
            _ => return None, // Unexpected response type for approval
        },
        ForwardedKind::Question => match parsed {
            ParsedEmailResponse::ChoiceIndex(idx) => InteractionResponsePayload::Answer {
                selected_choice: Some(idx),
                selected_choices: None,
                text: None,
            },
            ParsedEmailResponse::FreeformText(text) => InteractionResponsePayload::Answer {
                selected_choice: None,
                selected_choices: None,
                text: Some(text),
            },
            ParsedEmailResponse::Approve | ParsedEmailResponse::Deny => {
                // Treat as text answers for questions
                let text = match parsed {
                    ParsedEmailResponse::Approve => "yes",
                    ParsedEmailResponse::Deny => "no",
                    _ => unreachable!(),
                };
                InteractionResponsePayload::Answer {
                    selected_choice: None,
                    selected_choices: None,
                    text: Some(text.to_string()),
                }
            }
        },
    };
    Some(InboundResponse { request_id: request_id.to_string(), payload })
}

/// Build a response from a threaded text reply (Slack thread or Discord reply).
/// For approvals: "approve"/"yes" → approved, "deny"/"no" → denied.
/// For questions: try choice number or comma-separated numbers, otherwise treat as free-form answer.
fn build_text_response(
    request_id: &str,
    kind: ForwardedKind,
    body: &str,
) -> Option<InboundResponse> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    let payload = match kind {
        ForwardedKind::Approval => {
            let lower = trimmed.to_lowercase();
            if matches!(lower.as_str(), "approve" | "approved" | "yes" | "y" | "ok") {
                InteractionResponsePayload::ToolApproval {
                    approved: true,
                    allow_session: false,
                    allow_agent: false,
                }
            } else if matches!(lower.as_str(), "deny" | "denied" | "no" | "n" | "reject") {
                InteractionResponsePayload::ToolApproval {
                    approved: false,
                    allow_session: false,
                    allow_agent: false,
                }
            } else {
                return None; // Can't interpret this text as approval/denial
            }
        }
        ForwardedKind::Question => {
            // Try comma-separated choice numbers (e.g. "1,3,5") for multi-select
            if trimmed.contains(',') {
                let parsed: Vec<usize> = trimmed
                    .split(',')
                    .filter_map(|s| {
                        s.trim().parse::<usize>().ok().filter(|&n| n > 0).map(|n| n - 1)
                    })
                    .collect();
                if !parsed.is_empty() {
                    InteractionResponsePayload::Answer {
                        selected_choice: None,
                        selected_choices: Some(parsed),
                        text: None,
                    }
                } else {
                    InteractionResponsePayload::Answer {
                        selected_choice: None,
                        selected_choices: None,
                        text: Some(trimmed.to_string()),
                    }
                }
            }
            // Try as single choice number (1-indexed)
            else if let Ok(n) = trimmed.parse::<usize>() {
                if n > 0 {
                    InteractionResponsePayload::Answer {
                        selected_choice: Some(n - 1),
                        selected_choices: None,
                        text: None,
                    }
                } else {
                    InteractionResponsePayload::Answer {
                        selected_choice: None,
                        selected_choices: None,
                        text: Some(trimmed.to_string()),
                    }
                }
            } else {
                InteractionResponsePayload::Answer {
                    selected_choice: None,
                    selected_choices: None,
                    text: Some(trimmed.to_string()),
                }
            }
        }
    };
    Some(InboundResponse { request_id: request_id.to_string(), payload })
}

// ---------------------------------------------------------------------------
// Gate resolution
// ---------------------------------------------------------------------------

/// Resolve an interaction gate using the ChatManager.
async fn resolve_gate(
    chat: &ChatService,
    fi: &ForwardedInteraction,
    payload: InteractionResponsePayload,
) -> bool {
    let response = UserInteractionResponse { request_id: fi.request_id.clone(), payload };

    let result = if let Some(ref session_id) = fi.session_id {
        if fi.agent_id.is_empty() || fi.agent_id == *session_id {
            // Session-level interaction: resolve via the session's own interaction gate.
            chat.respond_to_interaction(session_id, response).await
        } else {
            // Spawned-agent interaction: resolve via the session's supervisor.
            chat.respond_to_agent_interaction(session_id, &fi.agent_id, response).await
        }
    } else {
        chat.respond_to_bot_interaction(&fi.agent_id, response).await
    };

    match result {
        Ok(true) => {
            info!(
                request_id = %fi.request_id,
                agent = %fi.agent_name,
                "AFK: resolved interaction gate via channel response"
            );
            true
        }
        Ok(false) => {
            debug!(
                request_id = %fi.request_id,
                "AFK: gate not acknowledged (already resolved?)"
            );
            false
        }
        Err(e) => {
            warn!(
                error = %e,
                request_id = %fi.request_id,
                "AFK: failed to resolve interaction gate"
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Message update after resolution
// ---------------------------------------------------------------------------

/// Update the original channel message to indicate the interaction was resolved.
/// Uses Slack chat.update / Discord message edit when possible, falls back to
/// sending a new message for email or when editing fails.
async fn update_channel_message(
    connectors: &ConnectorService,
    fi: &ForwardedInteraction,
    resolution: &str,
) {
    let update_text = format!("✅ {} — {}", resolution, fi.agent_name);

    // Try to edit the original message in-place.
    if !fi.channel_id.is_empty() && !fi.channel_message_id.is_empty() {
        match connectors
            .edit_message(&fi.connector_id, &fi.channel_id, &fi.channel_message_id, &update_text)
            .await
        {
            Ok(()) => return,
            Err(e) => {
                debug!(error = %e, "AFK: edit_message failed, falling back to new message");
            }
        }
    }

    // Fallback: send a new message (for email or when edit fails).
    if let Err(e) = connectors
        .send_message(
            &fi.connector_id,
            &[],
            None,
            &update_text,
            &[],
            None,
            fi.session_id.as_deref(),
        )
        .await
    {
        debug!(error = %e, "AFK: could not send resolution follow-up to channel");
    }
}

/// Acknowledge a Discord button interaction so the user's Discord UI shows success.
async fn ack_discord_interaction(
    connectors: &ConnectorService,
    fi: &ForwardedInteraction,
    metadata: &HashMap<String, String>,
    resolution: &str,
) {
    let interaction_id = match metadata.get("interaction_id") {
        Some(id) => id,
        None => return,
    };
    let interaction_token = match metadata.get("interaction_token") {
        Some(t) => t,
        None => return,
    };

    let update_text = format!("✅ {} — {}", resolution, fi.agent_name);
    if let Err(e) = connectors
        .acknowledge_interaction(&fi.connector_id, interaction_id, interaction_token, &update_text)
        .await
    {
        debug!(error = %e, "AFK: Discord interaction ACK failed");
    }
}

// ---------------------------------------------------------------------------
// Main forwarder task
// ---------------------------------------------------------------------------

/// Start the AFK forwarding background task.
///
/// This spawns a tokio task that:
/// 1. Subscribes to the approval broadcast channel for real-time approval events.
/// 2. Periodically polls for pending questions (no broadcast channel for those yet).
/// 3. When the user status is in `forward_on` list, formats and sends rich messages.
/// 4. Tracks forwarded interactions in the shared store.
/// 5. Subscribes to inbound connector messages to match and resolve forwarded gates.
pub fn spawn_afk_forwarder(
    config: Arc<ArcSwap<HiveMindConfig>>,
    user_status: UserStatusRuntime,
    chat: Arc<ChatService>,
    connectors: Option<Arc<ConnectorService>>,
    forwarded: ForwardedStore,
    event_bus: EventBus,
) -> tokio::task::JoinHandle<()> {
    // Subscribe to all inbound connector messages for response routing.
    let mut inbound_rx = event_bus.subscribe_queued_bounded("comm.message.received", 10_000);

    tokio::spawn(async move {
        info!("AFK forwarding service started");

        let mut approval_rx = chat.subscribe_approvals();
        let mut seen_approvals: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        // Track request_ids that failed to send so we don't retry every poll cycle.
        let mut send_failures: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        let mut question_poll_interval = tokio::time::interval(Duration::from_secs(10));
        question_poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                // Branch 1: Real-time approval events
                result = approval_rx.recv() => {
                    match result {
                        Ok(ApprovalStreamEvent::Added {
                            session_id,
                            agent_id,
                            agent_name,
                            request_id,
                            tool_id,
                            input,
                            reason,
                        }) => {
                            if seen_approvals.contains(&request_id) {
                                continue;
                            }
                            seen_approvals.insert(request_id.clone());

                            let status = user_status.get();
                            if !should_forward(&config, &user_status) {
                                let cfg = config.load();
                                info!(
                                    %status,
                                    has_channel = cfg.afk.forward_channel_id.is_some(),
                                    forward_on = ?cfg.afk.forward_on,
                                    "AFK forwarder: skipping approval (should_forward=false)"
                                );
                                continue;
                            }

                            info!(
                                %request_id,
                                %tool_id,
                                %status,
                                "AFK forwarder: forwarding approval"
                            );

                            let info = ForwardedInteractionInfo {
                                request_id: request_id.clone(),
                                agent_name: agent_name.clone(),
                                session_id: Some(session_id.clone()),
                                workflow_name: None,
                            };

                            if let Some(ref cs) = connectors {
                                forward_approval(
                                    &config.load(),
                                    cs,
                                    &tool_id,
                                    &input,
                                    &reason,
                                    &info,
                                    &agent_id,
                                    &agent_name,
                                    &forwarded,
                                )
                                .await;
                            } else {
                                warn!("AFK forwarder: no connector service available");
                            }
                        }
                        Ok(ApprovalStreamEvent::Resolved { request_id, .. }) => {
                            seen_approvals.remove(&request_id);
                            send_failures.remove(&request_id);
                            let removed = forwarded.lock().remove(&request_id);
                            if let (Some(fi), Some(ref cs)) = (removed, &connectors) {
                                update_channel_message(cs, &fi, "Resolved locally").await;
                            }
                        }
                        Ok(ApprovalStreamEvent::QuestionAdded { .. }) => {
                            // Questions are forwarded via the periodic poll branch.
                            continue;
                        }
                        Ok(ApprovalStreamEvent::Refresh) => {
                            // No-op for AFK forwarder — it uses periodic polling.
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            info!("AFK forwarder: approval broadcast closed, stopping");
                            break;
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            warn!(skipped = n, "AFK forwarder: lagged behind on approvals");
                            continue;
                        }
                    }
                }

                // Branch 2: Periodic question polling + auto-approve timeout check
                _ = question_poll_interval.tick() => {
                    // --- No-client AFK check ---
                    {
                        let cfg = config.load();
                        user_status.check_no_client_transition(&cfg.afk);
                    }

                    // --- Auto-approve timeout ---
                    let cfg = config.load();
                    if let Some(timeout_secs) = cfg.afk.auto_approve_on_timeout_secs {
                        if timeout_secs > 0 {
                            let timeout_dur = Duration::from_secs(timeout_secs);
                            let now = std::time::Instant::now();
                            // Collect expired approvals.
                            let expired: Vec<ForwardedInteraction> = {
                                let store = forwarded.lock();
                                store.values()
                                    .filter(|fi| {
                                        fi.kind == ForwardedKind::Approval
                                            && now.duration_since(fi.forwarded_at) >= timeout_dur
                                    })
                                    .cloned()
                                    .collect()
                            };
                            for fi in expired {
                                info!(
                                    request_id = %fi.request_id,
                                    elapsed_secs = timeout_secs,
                                    "AFK: auto-approving after timeout"
                                );
                                let payload = InteractionResponsePayload::ToolApproval {
                                    approved: true,
                                    allow_session: false,
                                    allow_agent: false,
                                };
                                let resolved = resolve_gate(&chat, &fi, payload).await;
                                if resolved {
                                    forwarded.lock().remove(&fi.request_id);
                                    if let Some(ref cs) = connectors {
                                        update_channel_message(cs, &fi, "Auto-approved (timeout)").await;
                                    }
                                }
                            }
                        }
                    }

                    // --- Question forwarding ---
                    let status = user_status.get();
                    if !should_forward(&config, &user_status) {
                        let c = config.load();
                        trace!(
                            %status,
                            has_channel = c.afk.forward_channel_id.is_some(),
                            forward_on = ?c.afk.forward_on,
                            "AFK forwarder: question poll skipped (should_forward=false)"
                        );
                        continue;
                    }
                    if !cfg.afk.forward_questions {
                        info!("AFK forwarder: forward_questions is disabled");
                        continue;
                    }

                    let questions = chat.list_all_pending_questions().await;
                    info!(count = questions.len(), "AFK forwarder: polled pending questions");
                    for (session_id, q) in questions {
                        let already = forwarded.lock().contains_key(&q.request_id);
                        if already || seen_approvals.contains(&q.request_id)
                            || send_failures.contains(&q.request_id)
                        {
                            continue;
                        }

                        let info = ForwardedInteractionInfo {
                            request_id: q.request_id.clone(),
                            agent_name: q.agent_name.clone(),
                            session_id: if session_id == "__bot__" {
                                None
                            } else {
                                Some(session_id.clone())
                            },
                            workflow_name: None,
                        };

                        if let Some(ref cs) = connectors {
                            let ok = forward_question(
                                &cfg,
                                cs,
                                &q.text,
                                &q.choices,
                                q.allow_freeform,
                                q.multi_select,
                                &info,
                                &q.agent_id,
                                &q.agent_name,
                                &forwarded,
                            )
                            .await;
                            if !ok {
                                send_failures.insert(q.request_id.clone());
                            }
                        }
                    }
                }

                // Branch 3: Inbound connector messages (button clicks, threaded replies, email replies)
                Some(envelope) = inbound_rx.recv() => {
                    if forwarded.lock().is_empty() {
                        continue;
                    }

                    let metadata: HashMap<String, String> = envelope.payload
                        .get("metadata")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default();
                    let body = envelope.payload
                        .get("body")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");

                    if let Some(inbound) = try_match_inbound(&metadata, body, &forwarded) {
                        let fi = forwarded.lock().get(&inbound.request_id).cloned();
                        if let Some(fi) = fi {
                            // Discord interaction ACK — must happen within 3 seconds.
                            if metadata.contains_key("interaction_id") {
                                if let Some(ref cs) = connectors {
                                    let desc = match fi.kind {
                                        ForwardedKind::Approval => "Approved via Discord",
                                        ForwardedKind::Question => "Answered via Discord",
                                    };
                                    ack_discord_interaction(cs, &fi, &metadata, desc).await;
                                }
                            }

                            let resolved = resolve_gate(&chat, &fi, inbound.payload).await;
                            if resolved {
                                forwarded.lock().remove(&inbound.request_id);
                                // For non-Discord channels, edit/follow-up the original message.
                                if !metadata.contains_key("interaction_id") {
                                    if let Some(ref cs) = connectors {
                                        let desc = match fi.kind {
                                            ForwardedKind::Approval => "Approved via channel",
                                            ForwardedKind::Question => "Answered via channel",
                                        };
                                        update_channel_message(cs, &fi, desc).await;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }.instrument(tracing::info_span!("service", service = "afk-forwarder")))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if we should forward interactions based on current status and config.
fn should_forward(config: &Arc<ArcSwap<HiveMindConfig>>, user_status: &UserStatusRuntime) -> bool {
    let cfg = config.load();
    let status = user_status.get();
    cfg.afk.forward_channel_id.is_some() && cfg.afk.forward_on.contains(&status)
}

/// Determine the channel type from the connector's provider.
fn detect_channel_type(connectors: &ConnectorService, connector_id: &str) -> Option<ChannelType> {
    use hive_contracts::connectors::ConnectorProvider;
    let list = connectors.list_connectors(None);
    let info = list.iter().find(|c| c.id == connector_id)?;
    match info.provider {
        ConnectorProvider::Slack => Some(ChannelType::Slack),
        ConnectorProvider::Discord => Some(ChannelType::Discord),
        ConnectorProvider::Gmail | ConnectorProvider::Imap | ConnectorProvider::Microsoft => {
            Some(ChannelType::Email)
        }
        _ => None,
    }
}

enum ChannelType {
    Slack,
    Discord,
    Email,
}

#[allow(clippy::too_many_arguments)]
async fn forward_approval(
    config: &HiveMindConfig,
    connectors: &ConnectorService,
    tool_id: &str,
    input: &str,
    reason: &str,
    info: &ForwardedInteractionInfo,
    agent_id: &str,
    agent_name: &str,
    store: &ForwardedStore,
) {
    if !config.afk.forward_approvals {
        info!("AFK forwarder: forward_approvals is disabled, skipping");
        return;
    }
    let connector_id = match &config.afk.forward_channel_id {
        Some(id) => id.clone(),
        None => {
            info!("AFK forwarder: no forward_channel_id configured");
            return;
        }
    };

    let channel_type = match detect_channel_type(connectors, &connector_id) {
        Some(ct) => ct,
        None => {
            warn!(connector_id, "AFK forwarder: unknown channel type for connector");
            return;
        }
    };

    let to_addr = config.afk.forward_to_address.as_deref().unwrap_or("<none>");
    info!(
        %connector_id,
        channel_type = match channel_type { ChannelType::Slack => "slack", ChannelType::Discord => "discord", ChannelType::Email => "email" },
        to_addr,
        "AFK forwarder: sending approval via connector"
    );

    let formatted = match channel_type {
        ChannelType::Slack => {
            interaction_format::format_approval_slack(tool_id, input, reason, info)
        }
        ChannelType::Discord => {
            interaction_format::format_approval_discord(tool_id, input, reason, info)
        }
        ChannelType::Email => {
            interaction_format::format_approval_html(tool_id, input, reason, info)
        }
    };

    send_formatted(
        connectors,
        &connector_id,
        config.afk.forward_to_address.as_deref(),
        &formatted,
        agent_id,
        agent_name,
        info,
        ForwardedKind::Approval,
        store,
    )
    .await;
}

#[allow(clippy::too_many_arguments)]
async fn forward_question(
    config: &HiveMindConfig,
    connectors: &ConnectorService,
    text: &str,
    choices: &[String],
    allow_freeform: bool,
    multi_select: bool,
    info: &ForwardedInteractionInfo,
    agent_id: &str,
    agent_name: &str,
    store: &ForwardedStore,
) -> bool {
    let connector_id = match &config.afk.forward_channel_id {
        Some(id) => id.clone(),
        None => return false,
    };

    let channel_type = match detect_channel_type(connectors, &connector_id) {
        Some(ct) => ct,
        None => {
            warn!(connector_id, "AFK forwarder: unknown channel type for connector");
            return false;
        }
    };

    let formatted = match channel_type {
        ChannelType::Slack => interaction_format::format_question_slack(
            text,
            choices,
            allow_freeform,
            multi_select,
            info,
        ),
        ChannelType::Discord => interaction_format::format_question_discord(
            text,
            choices,
            allow_freeform,
            multi_select,
            info,
        ),
        ChannelType::Email => interaction_format::format_question_html(
            text,
            choices,
            allow_freeform,
            multi_select,
            info,
        ),
    };

    send_formatted(
        connectors,
        &connector_id,
        config.afk.forward_to_address.as_deref(),
        &formatted,
        agent_id,
        agent_name,
        info,
        ForwardedKind::Question,
        store,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn send_formatted(
    connectors: &ConnectorService,
    connector_id: &str,
    forward_to_address: Option<&str>,
    formatted: &FormattedInteraction,
    agent_id: &str,
    agent_name: &str,
    info: &ForwardedInteractionInfo,
    kind: ForwardedKind,
    store: &ForwardedStore,
) -> bool {
    let recipients: Vec<String> = forward_to_address
        .filter(|s| !s.is_empty())
        .map(|s| vec![s.to_string()])
        .unwrap_or_default();
    match connectors
        .send_rich_message(
            connector_id,
            &recipients,
            Some(&formatted.subject),
            &formatted.fallback_text,
            Some(formatted.rich_body.clone()),
            &[],
            Some(agent_id),
            info.session_id.as_deref(),
        )
        .await
    {
        Ok(msg) => {
            let channel_msg_id = msg.metadata.get("external_id").cloned().unwrap_or_default();

            // Resolve the channel_id — either from `to` or the connector's default.
            let channel_id = msg
                .to
                .first()
                .cloned()
                .or_else(|| connectors.default_send_channel_id(connector_id))
                .unwrap_or_default();

            // For Slack, the message ts IS the thread_ts for replies.
            // For Discord, the message ID is what reply references point to.
            let thread_id =
                if !channel_msg_id.is_empty() { Some(channel_msg_id.clone()) } else { None };

            debug!(
                request_id = %info.request_id,
                channel_msg_id,
                channel_id,
                "AFK forwarder: sent interaction to channel"
            );

            store.lock().insert(
                info.request_id.clone(),
                ForwardedInteraction {
                    request_id: info.request_id.clone(),
                    connector_id: connector_id.to_string(),
                    channel_id,
                    channel_message_id: channel_msg_id,
                    thread_id,
                    session_id: info.session_id.clone(),
                    agent_id: agent_id.to_string(),
                    agent_name: agent_name.to_string(),
                    workflow_name: info.workflow_name.clone(),
                    kind,
                    forwarded_at: std::time::Instant::now(),
                },
            );
            true
        }
        Err(e) => {
            warn!(
                error = ?e,
                request_id = %info.request_id,
                "AFK forwarder: failed to send interaction to channel"
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_email_reply_approve() {
        assert!(matches!(
            parse_email_reply_body("approve\n\n> On Jan 1..."),
            Some(ParsedEmailResponse::Approve)
        ));
        assert!(matches!(parse_email_reply_body("  Yes  \n"), Some(ParsedEmailResponse::Approve)));
    }

    #[test]
    fn test_parse_email_reply_deny() {
        assert!(matches!(parse_email_reply_body("deny"), Some(ParsedEmailResponse::Deny)));
        assert!(matches!(
            parse_email_reply_body("no\n> original message"),
            Some(ParsedEmailResponse::Deny)
        ));
    }

    #[test]
    fn test_parse_email_reply_choice() {
        assert!(matches!(
            parse_email_reply_body("3\n"),
            Some(ParsedEmailResponse::ChoiceIndex(2)) // 1-indexed → 0-indexed
        ));
    }

    #[test]
    fn test_parse_email_reply_freeform() {
        assert!(matches!(
            parse_email_reply_body("Use the second option please\n> ..."),
            Some(ParsedEmailResponse::FreeformText(ref s)) if s == "Use the second option please"
        ));
    }

    #[test]
    fn test_parse_email_reply_empty() {
        assert!(parse_email_reply_body("").is_none());
        assert!(parse_email_reply_body("> quoted only").is_none());
    }

    #[test]
    fn test_parse_hivemind_action_approve() {
        let result = parse_hivemind_action("hivemind_approve:req-123").unwrap();
        assert_eq!(result.request_id, "req-123");
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::ToolApproval { approved: true, .. }
        ));
    }

    #[test]
    fn test_parse_hivemind_action_deny() {
        let result = parse_hivemind_action("hivemind_deny:req-456").unwrap();
        assert_eq!(result.request_id, "req-456");
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::ToolApproval { approved: false, .. }
        ));
    }

    #[test]
    fn test_parse_hivemind_action_choice() {
        let result = parse_hivemind_action("hivemind_choice:req-789:2").unwrap();
        assert_eq!(result.request_id, "req-789");
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::Answer { selected_choice: Some(2), .. }
        ));
    }

    #[test]
    fn test_parse_hivemind_action_unknown() {
        assert!(parse_hivemind_action("some_other_action").is_none());
    }

    #[test]
    fn test_build_text_response_approval_approve() {
        let result = build_text_response("req-1", ForwardedKind::Approval, "approve").unwrap();
        assert_eq!(result.request_id, "req-1");
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::ToolApproval { approved: true, .. }
        ));
    }

    #[test]
    fn test_build_text_response_approval_deny() {
        let result = build_text_response("req-2", ForwardedKind::Approval, "no").unwrap();
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::ToolApproval { approved: false, .. }
        ));
    }

    #[test]
    fn test_build_text_response_approval_unrecognized() {
        assert!(build_text_response("req-3", ForwardedKind::Approval, "maybe later").is_none());
    }

    #[test]
    fn test_build_text_response_question_choice() {
        let result = build_text_response("req-4", ForwardedKind::Question, "2").unwrap();
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::Answer { selected_choice: Some(1), .. }
        ));
    }

    #[test]
    fn test_build_text_response_question_freeform() {
        let result =
            build_text_response("req-5", ForwardedKind::Question, "Use the blue option").unwrap();
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::Answer { selected_choice: None, text: Some(ref t), .. } if t == "Use the blue option"
        ));
    }

    #[test]
    fn test_build_text_response_empty() {
        assert!(build_text_response("req-6", ForwardedKind::Approval, "  ").is_none());
    }

    #[test]
    fn test_build_text_response_multi_select_comma_separated() {
        let result = build_text_response("req-7", ForwardedKind::Question, "1,3,5").unwrap();
        assert_eq!(result.request_id, "req-7");
        match result.payload {
            InteractionResponsePayload::Answer { selected_choice, selected_choices, text } => {
                assert!(selected_choice.is_none());
                assert_eq!(selected_choices, Some(vec![0, 2, 4])); // 1-indexed → 0-indexed
                assert!(text.is_none());
            }
            _ => panic!("expected Answer payload"),
        }
    }

    #[test]
    fn test_build_text_response_multi_select_with_spaces() {
        let result = build_text_response("req-8", ForwardedKind::Question, "2, 4").unwrap();
        match result.payload {
            InteractionResponsePayload::Answer { selected_choices, .. } => {
                assert_eq!(selected_choices, Some(vec![1, 3]));
            }
            _ => panic!("expected Answer payload"),
        }
    }

    // --- HTML stripping tests ---

    #[test]
    fn test_strip_html_basic_tags() {
        let html = "<html><body><p>Hello</p><p>World</p></body></html>";
        let text = strip_html_to_text(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains('<'));
    }

    #[test]
    fn test_strip_html_br_tags() {
        let html = "Line one<br>Line two<br/>Line three";
        let text = strip_html_to_text(html);
        assert!(text.contains("Line one\nLine two\nLine three"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "A &amp; B &lt; C &gt; D &quot;E&quot;";
        let text = strip_html_to_text(html);
        assert_eq!(text.trim(), "A & B < C > D \"E\"");
    }

    #[test]
    fn test_strip_html_style_block() {
        let html = "<style>body { color: red; }</style><p>Visible</p>";
        let text = strip_html_to_text(html);
        assert!(text.contains("Visible"));
        assert!(!text.contains("color"));
    }

    #[test]
    fn test_strip_html_realistic_email_reply() {
        // Simulate what Microsoft Graph returns for an HTML email reply
        let html = r#"<html><head>
<style>/* outlook styles */</style>
</head><body>
<div>yes</div>
<div id="appendonsend"></div>
<hr>
<div id="divRplyFwdMsg">
<b>From:</b> HiveMind OS Agent<br>
<b>Subject:</b> [HIVEMIND:question-abc123] Question from assistant
<div><p>What color do you prefer?</p></div>
</div>
</body></html>"#;
        let text = strip_html_to_text(html);
        let parsed = parse_email_reply_body(&text);
        assert!(
            matches!(parsed, Some(ParsedEmailResponse::Approve)),
            "Expected 'yes' to parse as Approve, got: {parsed:?}\nStripped text:\n{text}"
        );
    }

    #[test]
    fn test_looks_like_html() {
        assert!(looks_like_html("<html><body>hello</body></html>"));
        assert!(looks_like_html("  <div>hello</div>"));
        assert!(looks_like_html("<!DOCTYPE html><html>"));
        assert!(!looks_like_html("Just plain text"));
        assert!(!looks_like_html("yes"));
        assert!(!looks_like_html("> quoted reply"));
    }

    #[test]
    fn test_build_email_response_html_body() {
        let html = "<html><body><div>approve</div><hr><div>original question</div></body></html>";
        let result = build_email_response("req-7", ForwardedKind::Approval, html).unwrap();
        assert!(matches!(
            result.payload,
            InteractionResponsePayload::ToolApproval { approved: true, .. }
        ));
    }
}
