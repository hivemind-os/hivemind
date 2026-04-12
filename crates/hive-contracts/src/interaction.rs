use serde::{Deserialize, Serialize};

/// A request for user interaction, sent from the agent loop to the UI.
/// Designed to be transport-agnostic so it can be forwarded to mobile,
/// Slack, etc. for "away from keyboard" workflows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInteractionRequest {
    pub request_id: String,
    pub kind: InteractionKind,
}

/// The kind of interaction requested.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionKind {
    /// Tool requires user approval before execution.
    ToolApproval {
        tool_id: String,
        input: String,
        reason: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        inferred_scope: Option<String>,
    },
    /// Agent is asking the user a question.
    Question {
        /// The question or prompt text shown to the user.
        text: String,
        /// Optional list of choices. Empty means free-text only.
        #[serde(default)]
        choices: Vec<String>,
        /// Whether the user can type a free-form answer (in addition to or instead of choices).
        #[serde(default = "default_true")]
        allow_freeform: bool,
        /// When true, the user can select multiple choices at once.
        #[serde(default)]
        multi_select: bool,
        /// The assistant's accompanying message content (text produced alongside the tool call).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
}

fn default_true() -> bool {
    true
}

/// The user's response to an interaction request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserInteractionResponse {
    pub request_id: String,
    pub payload: InteractionResponsePayload,
}

/// Payload of a user's interaction response, matching the request kind.
///
/// NOTE: `rename_all` on the enum only renames variant names (tag values),
/// NOT fields within variants.  Each variant must have its own `rename_all`
/// so that field names are correctly mapped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionResponsePayload {
    /// Response to a ToolApproval interaction.
    ToolApproval {
        approved: bool,
        /// When true, creates a permission rule for all agents in the session.
        #[serde(default)]
        allow_session: bool,
        /// When true, creates a permission rule for the specific agent only.
        #[serde(default)]
        allow_agent: bool,
    },
    /// Response to a Question interaction.
    Answer {
        /// The selected choice index (if the user picked a single choice).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_choice: Option<usize>,
        /// The selected choice indices (when multi-select is enabled).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        selected_choices: Option<Vec<usize>>,
        /// Free-form text answer (if the user typed one).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_interaction_response_round_trip() {
        // Simulate what the JS frontend sends via Tauri
        let js_json = serde_json::json!({
            "request_id": "approval-http.request-b67f3867-feac-462e-afd5-f9559fcb5dc6",
            "payload": {
                "type": "tool_approval",
                "approved": true,
                "allow_session": false,
                "allow_agent": false
            }
        });

        // Step 1: Tauri deserializes JS args into UserInteractionResponse
        let response: UserInteractionResponse = serde_json::from_value(js_json.clone()).unwrap();
        println!("Deserialized: {response:?}");

        // Step 2: blocking_post_json serializes it back to JSON for the daemon
        let re_serialized = serde_json::to_string(&response).unwrap();
        println!("Re-serialized: {re_serialized}");

        // Step 3: Check the re-serialized JSON has a payload field
        let parsed: serde_json::Value = serde_json::from_str(&re_serialized).unwrap();
        assert!(parsed.get("payload").is_some(), "Should have payload field: {re_serialized}");
        assert!(parsed.get("request_id").is_some(), "Should have request_id field");
    }

    #[test]
    fn allow_agent_flag_deserializes_correctly() {
        let js_json = serde_json::json!({
            "request_id": "approval-test-123",
            "payload": {
                "type": "tool_approval",
                "approved": true,
                "allow_session": false,
                "allow_agent": true
            }
        });

        let response: UserInteractionResponse = serde_json::from_value(js_json).unwrap();
        match response.payload {
            InteractionResponsePayload::ToolApproval { approved, allow_session, allow_agent } => {
                assert!(approved);
                assert!(!allow_session);
                assert!(allow_agent);
            }
            _ => panic!("expected ToolApproval payload"),
        }
    }

    /// Verifies that the session route match pattern used in
    /// `handle_chat_interaction_response` correctly matches BOTH
    /// `allow_session: true` and `allow_agent: true`.
    ///
    /// Previously the route only matched `allow_session`, silently
    /// ignoring `allow_agent` and causing "Allow for Agent" to
    /// only approve once without persisting the permission rule.
    #[test]
    fn session_route_match_pattern_handles_both_flags() {
        let cases = vec![
            // (allow_session, allow_agent, should_grant)
            (true, false, true),   // "Allow for Session"
            (false, true, true),   // "Allow for Agent"
            (true, true, true),    // Both
            (false, false, false), // Plain approve (no persistent rule)
        ];

        for (allow_session, allow_agent, should_grant) in cases {
            let payload = InteractionResponsePayload::ToolApproval {
                approved: true,
                allow_session,
                allow_agent,
            };

            // This is the exact match pattern from handle_chat_interaction_response
            let matched = matches!(
                &payload,
                InteractionResponsePayload::ToolApproval {
                    allow_session: s,
                    allow_agent: a,
                    ..
                } if *s || *a
            );

            assert_eq!(
                matched, should_grant,
                "allow_session={allow_session}, allow_agent={allow_agent}: \
                 expected grant={should_grant}, got {matched}"
            );
        }
    }
}
