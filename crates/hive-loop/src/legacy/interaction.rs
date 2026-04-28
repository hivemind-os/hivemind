use std::collections::HashMap;

use hive_contracts::{InteractionKind, UserInteractionResponse};
use parking_lot::Mutex;
use tokio::sync::oneshot;

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

#[cfg(test)]
mod tests {
    use super::*;
    use hive_contracts::InteractionResponsePayload;

    fn question_kind(text: &str) -> InteractionKind {
        InteractionKind::Question {
            text: text.to_string(),
            choices: vec![],
            allow_freeform: true,
            multi_select: false,
            message: None,
        }
    }

    #[tokio::test]
    async fn create_request_and_respond_round_trip() {
        let gate = UserInteractionGate::new();
        let rx = gate.create_request("req-1".to_string(), question_kind("Hello?"));

        assert_eq!(gate.list_pending().len(), 1);

        let found = gate.respond(UserInteractionResponse {
            request_id: "req-1".to_string(),
            payload: InteractionResponsePayload::Answer {
                selected_choice: None,
                selected_choices: None,
                text: Some("world".to_string()),
            },
        });
        assert!(found);

        let resp = rx.await.unwrap();
        match resp.payload {
            InteractionResponsePayload::Answer { text, .. } => {
                assert_eq!(text, Some("world".to_string()));
            }
            _ => panic!("unexpected payload"),
        }

        assert!(gate.list_pending().is_empty());
    }

    #[test]
    fn respond_returns_false_for_unknown_id() {
        let gate = UserInteractionGate::new();
        let found = gate.respond(UserInteractionResponse {
            request_id: "nonexistent".to_string(),
            payload: InteractionResponsePayload::Answer {
                selected_choice: None,
                selected_choices: None,
                text: None,
            },
        });
        assert!(!found);
    }

    #[tokio::test]
    async fn close_unblocks_pending_receivers() {
        let gate = UserInteractionGate::new();
        let rx1 = gate.create_request("req-1".to_string(), question_kind("Q1"));
        let rx2 = gate.create_request("req-2".to_string(), question_kind("Q2"));

        gate.close();

        // Receivers should get RecvError since senders were dropped
        assert!(rx1.await.is_err());
        assert!(rx2.await.is_err());
        assert!(gate.list_pending().is_empty());
    }

    #[test]
    fn inject_pending_makes_request_visible() {
        let gate = UserInteractionGate::new();
        gate.inject_pending("injected-1".to_string(), question_kind("Injected"));

        let pending = gate.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "injected-1");
    }

    #[test]
    fn remove_all_except_keeps_only_specified() {
        let gate = UserInteractionGate::new();
        gate.inject_pending("keep".to_string(), question_kind("Keep"));
        gate.inject_pending("stale-1".to_string(), question_kind("Stale 1"));
        gate.inject_pending("stale-2".to_string(), question_kind("Stale 2"));

        let removed = gate.remove_all_except("keep");
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"stale-1".to_string()));
        assert!(removed.contains(&"stale-2".to_string()));

        let pending = gate.list_pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].0, "keep");
    }

    #[tokio::test]
    async fn drop_unblocks_pending_receivers() {
        let rx = {
            let gate = UserInteractionGate::new();
            gate.create_request("req-1".to_string(), question_kind("Q"))
            // gate is dropped here
        };

        // Receiver should get RecvError since sender was dropped
        assert!(rx.await.is_err());
    }

    #[test]
    fn get_pending_kind_returns_correct_kind() {
        let gate = UserInteractionGate::new();
        gate.inject_pending("req-1".to_string(), question_kind("What?"));

        let kind = gate.get_pending_kind("req-1");
        assert!(kind.is_some());
        assert!(gate.get_pending_kind("nonexistent").is_none());
    }
}
