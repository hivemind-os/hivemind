//! Formatting functions that convert `InteractionKind` into rich message
//! bodies for Slack Block Kit, Discord Components, and HTML email.

use super::communication::RichMessageBody;
use serde_json::json;

/// Information about an interaction to be forwarded.
pub struct ForwardedInteractionInfo {
    pub request_id: String,
    pub agent_name: String,
    pub session_id: Option<String>,
    pub workflow_name: Option<String>,
}

/// Result of formatting an interaction for a specific channel type.
pub struct FormattedInteraction {
    /// Plain-text fallback for notifications / screen readers.
    pub fallback_text: String,
    /// Email subject line (with `[HIVEMIND:<token>]` for reply matching).
    pub subject: String,
    /// The rich body to pass to `send_rich()`.
    pub rich_body: RichMessageBody,
}

// ---------------------------------------------------------------------------
// Slack Block Kit
// ---------------------------------------------------------------------------

/// Format a tool approval request as Slack Block Kit blocks.
pub fn format_approval_slack(
    tool_id: &str,
    input: &str,
    reason: &str,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let header = match &info.workflow_name {
        Some(wf) => format!("🔐 Tool Approval — {wf} / {}", info.agent_name),
        None => format!("🔐 Tool Approval — {}", info.agent_name),
    };

    // Truncate input preview for Slack's 3000-char block limit.
    let input_preview = truncate(input, 2000);

    let blocks = json!([
        {
            "type": "header",
            "text": { "type": "plain_text", "text": header, "emoji": true }
        },
        {
            "type": "section",
            "fields": [
                { "type": "mrkdwn", "text": format!("*Tool:*\n`{tool_id}`") },
                { "type": "mrkdwn", "text": format!("*Reason:*\n{reason}") }
            ]
        },
        {
            "type": "section",
            "text": {
                "type": "mrkdwn",
                "text": format!("*Input:*\n```{}```", input_preview)
            }
        },
        {
            "type": "actions",
            "elements": [
                {
                    "type": "button",
                    "text": { "type": "plain_text", "text": "✅ Approve", "emoji": true },
                    "style": "primary",
                    "action_id": format!("hivemind_approve:{}", info.request_id),
                    "value": info.request_id.clone(),
                },
                {
                    "type": "button",
                    "text": { "type": "plain_text", "text": "❌ Deny", "emoji": true },
                    "style": "danger",
                    "action_id": format!("hivemind_deny:{}", info.request_id),
                    "value": info.request_id.clone(),
                }
            ]
        }
    ]);

    FormattedInteraction {
        fallback_text: format!("🔐 Approve {tool_id}? {reason}"),
        subject: format!("[HIVEMIND:{}] Tool Approval: {}", short_id(&info.request_id), tool_id),
        rich_body: RichMessageBody::SlackBlocks(blocks),
    }
}

/// Format a question as Slack Block Kit blocks.
pub fn format_question_slack(
    text: &str,
    choices: &[String],
    allow_freeform: bool,
    multi_select: bool,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let header = match &info.workflow_name {
        Some(wf) => format!("❓ Question — {wf} / {}", info.agent_name),
        None => format!("❓ Question — {}", info.agent_name),
    };

    let mut blocks = vec![
        json!({
            "type": "header",
            "text": { "type": "plain_text", "text": header, "emoji": true }
        }),
        json!({
            "type": "section",
            "text": { "type": "mrkdwn", "text": text }
        }),
    ];

    if !choices.is_empty() {
        if multi_select {
            // Render choices as a numbered list for multi-select.
            let numbered: String = choices
                .iter()
                .enumerate()
                .map(|(i, c)| format!("{}. {}", i + 1, c))
                .collect::<Vec<_>>()
                .join("\n");
            blocks.push(json!({
                "type": "section",
                "text": { "type": "mrkdwn", "text": numbered }
            }));
            blocks.push(json!({
                "type": "context",
                "elements": [{
                    "type": "mrkdwn",
                    "text": "_Reply in this thread with comma-separated choice numbers (e.g. 1,3,5)_"
                }]
            }));
        } else {
            let mut elements: Vec<serde_json::Value> = choices
                .iter()
                .enumerate()
                .map(|(i, choice)| {
                    json!({
                        "type": "button",
                        "text": { "type": "plain_text", "text": truncate(choice, 75), "emoji": true },
                        "action_id": format!("hivemind_choice:{}:{}", info.request_id, i),
                        "value": format!("{}:{}", i, choice),
                    })
                })
                .collect();

            // Slack allows max 25 elements per action block, split if needed.
            while !elements.is_empty() {
                let chunk: Vec<_> = elements.drain(..elements.len().min(25)).collect();
                blocks.push(json!({ "type": "actions", "elements": chunk }));
            }
        }
    }

    if allow_freeform && !choices.is_empty() && !multi_select {
        blocks.push(json!({
            "type": "context",
            "elements": [{
                "type": "mrkdwn",
                "text": "_You can also reply in this thread with a free-form answer._"
            }]
        }));
    }

    FormattedInteraction {
        fallback_text: format!("❓ {text}"),
        subject: format!(
            "[HIVEMIND:{}] Question from {}",
            short_id(&info.request_id),
            info.agent_name
        ),
        rich_body: RichMessageBody::SlackBlocks(json!(blocks)),
    }
}

// ---------------------------------------------------------------------------
// Discord Components
// ---------------------------------------------------------------------------

/// Format a tool approval request as Discord message components.
pub fn format_approval_discord(
    tool_id: &str,
    input: &str,
    reason: &str,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let input_preview = truncate(input, 1500);
    let content = match &info.workflow_name {
        Some(wf) => format!(
            "🔐 **Tool Approval** — {wf} / {}\n**Tool:** `{tool_id}`\n**Reason:** {reason}\n**Input:**\n```json\n{input_preview}\n```",
            info.agent_name
        ),
        None => format!(
            "🔐 **Tool Approval** — {}\n**Tool:** `{tool_id}`\n**Reason:** {reason}\n**Input:**\n```json\n{input_preview}\n```",
            info.agent_name
        ),
    };

    let components = json!([{
        "type": 1, // ACTION_ROW
        "components": [
            {
                "type": 2, // BUTTON
                "style": 3, // SUCCESS (green)
                "label": "✅ Approve",
                "custom_id": format!("hivemind_approve:{}", info.request_id),
            },
            {
                "type": 2,
                "style": 4, // DANGER (red)
                "label": "❌ Deny",
                "custom_id": format!("hivemind_deny:{}", info.request_id),
            }
        ]
    }]);

    FormattedInteraction {
        fallback_text: content.clone(),
        subject: format!("[HIVEMIND:{}] Tool Approval: {}", short_id(&info.request_id), tool_id),
        rich_body: RichMessageBody::DiscordComponents(components),
    }
}

/// Format a question as Discord message components.
pub fn format_question_discord(
    text: &str,
    choices: &[String],
    allow_freeform: bool,
    multi_select: bool,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let mut content = match &info.workflow_name {
        Some(wf) => format!("❓ **Question** — {wf} / {}\n{text}", info.agent_name),
        None => format!("❓ **Question** — {}\n{text}", info.agent_name),
    };

    let mut components = Vec::new();

    if !choices.is_empty() && multi_select {
        // Render choices as a numbered list in the content text.
        let numbered: String = choices
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{}. {}", i + 1, c))
            .collect::<Vec<_>>()
            .join("\n");
        content.push_str(&format!("\n{numbered}"));
        content
            .push_str("\n_Reply to this message with comma-separated choice numbers (e.g. 1,3,5)_");
        // No button components for multi-select.
    } else {
        // Discord allows max 5 buttons per action row, max 5 rows.
        let chunks: Vec<Vec<serde_json::Value>> = choices
            .iter()
            .enumerate()
            .map(|(i, choice)| {
                json!({
                    "type": 2,
                    "style": 1, // PRIMARY
                    "label": truncate(choice, 80),
                    "custom_id": format!("hivemind_choice:{}:{}", info.request_id, i),
                })
            })
            .collect::<Vec<_>>()
            .chunks(5)
            .map(|c| c.to_vec())
            .collect();

        for row_buttons in chunks.into_iter().take(5) {
            components.push(json!({
                "type": 1,
                "components": row_buttons,
            }));
        }

        if allow_freeform && !choices.is_empty() {
            content.push_str("\n_You can also reply to this message with a free-form answer._");
        }
    }

    FormattedInteraction {
        fallback_text: content.clone(),
        subject: format!(
            "[HIVEMIND:{}] Question from {}",
            short_id(&info.request_id),
            info.agent_name
        ),
        rich_body: RichMessageBody::DiscordComponents(json!(components)),
    }
}

// ---------------------------------------------------------------------------
// HTML Email
// ---------------------------------------------------------------------------

/// Format a tool approval request as an HTML email body.
pub fn format_approval_html(
    tool_id: &str,
    input: &str,
    reason: &str,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let input_escaped = html_escape(input);
    let reason_escaped = html_escape(reason);
    let tool_escaped = html_escape(tool_id);
    let agent_escaped = html_escape(&info.agent_name);

    let context_line = match &info.workflow_name {
        Some(wf) => format!("{} / {}", html_escape(wf), agent_escaped),
        None => agent_escaped.clone(),
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html><body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #d97706;">🔐 Tool Approval Required</h2>
<p style="color: #6b7280; font-size: 14px;">{context_line}</p>
<table style="border-collapse: collapse; width: 100%; margin: 16px 0;">
  <tr><td style="padding: 8px 12px; background: #f3f4f6; font-weight: bold; width: 80px;">Tool</td><td style="padding: 8px 12px;"><code>{tool_escaped}</code></td></tr>
  <tr><td style="padding: 8px 12px; background: #f3f4f6; font-weight: bold;">Reason</td><td style="padding: 8px 12px;">{reason_escaped}</td></tr>
</table>
<details><summary style="cursor: pointer; font-weight: bold; margin: 12px 0;">Input</summary>
<pre style="background: #1e1e1e; color: #d4d4d4; padding: 12px; border-radius: 6px; overflow-x: auto; font-size: 13px;">{input_escaped}</pre>
</details>
<hr style="border: none; border-top: 1px solid #e5e7eb; margin: 24px 0;">
<p style="font-size: 14px;"><strong>Reply to this email</strong> with one of the following on the first line:</p>
<ul style="font-size: 14px;">
  <li><code>approve</code> — approve this tool execution</li>
  <li><code>deny</code> — deny this tool execution</li>
</ul>
<p style="color: #9ca3af; font-size: 12px;">Request ID: {}</p>
</body></html>"#,
        info.request_id
    );

    let subject = format!("[HIVEMIND:{}] Tool Approval: {}", short_id(&info.request_id), tool_id);

    FormattedInteraction {
        fallback_text: format!(
            "Tool Approval Required\nTool: {tool_id}\nReason: {reason}\nReply with 'approve' or 'deny'."
        ),
        subject,
        rich_body: RichMessageBody::Html(html),
    }
}

/// Format a question as an HTML email body.
pub fn format_question_html(
    text: &str,
    choices: &[String],
    allow_freeform: bool,
    multi_select: bool,
    info: &ForwardedInteractionInfo,
) -> FormattedInteraction {
    let text_escaped = html_escape(text);
    let agent_escaped = html_escape(&info.agent_name);

    let context_line = match &info.workflow_name {
        Some(wf) => format!("{} / {}", html_escape(wf), agent_escaped),
        None => agent_escaped.clone(),
    };

    let choices_html = if choices.is_empty() {
        String::new()
    } else {
        let items: String = choices
            .iter()
            .enumerate()
            .map(|(i, c)| format!("<li><code>{}</code> — {}</li>", i + 1, html_escape(c)))
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            r#"<p style="font-size: 14px;"><strong>Choices:</strong></p>
<ol style="font-size: 14px;">{items}</ol>"#
        )
    };

    let reply_instructions = if multi_select && !choices.is_empty() {
        "<p style=\"font-size: 14px;\"><strong>Reply to this email</strong> with comma-separated choice numbers (e.g. 1,3,5).</p>"
    } else if choices.is_empty() || allow_freeform {
        "<p style=\"font-size: 14px;\"><strong>Reply to this email</strong> with your answer on the first line.</p>"
    } else {
        "<p style=\"font-size: 14px;\"><strong>Reply to this email</strong> with the choice number on the first line.</p>"
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html><body style="font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; max-width: 600px; margin: 0 auto; padding: 20px;">
<h2 style="color: #2563eb;">❓ Question</h2>
<p style="color: #6b7280; font-size: 14px;">{context_line}</p>
<p style="font-size: 16px; margin: 16px 0;">{text_escaped}</p>
{choices_html}
<hr style="border: none; border-top: 1px solid #e5e7eb; margin: 24px 0;">
{reply_instructions}
<p style="color: #9ca3af; font-size: 12px;">Request ID: {}</p>
</body></html>"#,
        info.request_id
    );

    let subject =
        format!("[HIVEMIND:{}] Question from {}", short_id(&info.request_id), info.agent_name);

    FormattedInteraction {
        fallback_text: format!("Question from {}: {text}", info.agent_name),
        subject,
        rich_body: RichMessageBody::Html(html),
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract first 8 chars of a request ID as a short token for subject matching.
fn short_id(request_id: &str) -> &str {
    &request_id[..request_id.len().min(8)]
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Find a valid UTF-8 boundary
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
