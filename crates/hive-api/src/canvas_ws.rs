use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// Re-export data structures from hive-chat so existing consumers see them here.
pub use hive_chat::canvas_ws::{
    CanvasSession, CanvasSessionRegistry, SequencedEvent, ServerMessage,
};

/// Client → Server messages
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    PositionUpdate { card_id: String, x: f64, y: f64 },
    PromptSubmit { content: String, x: f64, y: f64 },
}

// ---------------------------------------------------------------------------
// WebSocket handler
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct WsQuery {
    pub last_sequence: Option<u64>,
}

pub async fn canvas_ws_handler(
    ws: WebSocketUpgrade,
    Path(session_id): Path<String>,
    Query(query): Query<WsQuery>,
    State(state): State<super::AppState>,
) -> impl IntoResponse {
    let session = state.canvas_sessions.get_or_create(&session_id);
    ws.on_upgrade(move |socket| handle_canvas_socket(socket, session, query.last_sequence))
}

async fn handle_canvas_socket(
    socket: WebSocket,
    session: Arc<CanvasSession>,
    last_sequence: Option<u64>,
) {
    let client_id = uuid::Uuid::new_v4().to_string();

    let (mut sender, mut receiver) = socket.split();

    // 1. Send Welcome
    let welcome = ServerMessage::Welcome {
        client_id: client_id.clone(),
        sequence: session.sequence.load(std::sync::atomic::Ordering::Relaxed),
    };
    if let Ok(text) = serde_json::to_string(&welcome) {
        if sender.send(Message::Text(text.into())).await.is_err() {
            return;
        }
    }

    // 2. Send Replay if the client provided a last_sequence
    if let Some(last_seq) = last_sequence {
        let events = session.replay_from(last_seq);
        if !events.is_empty() {
            let replay = ServerMessage::Replay { events };
            if let Ok(text) = serde_json::to_string(&replay) {
                if sender.send(Message::Text(text.into())).await.is_err() {
                    return;
                }
            }
        }
    }

    // 3. Subscribe to broadcast channel for live events
    let mut rx = session.broadcast_tx.subscribe();

    // Forward broadcast events to this client
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(text) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(text.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // 4. Handle incoming messages from the client
    let session_for_recv = session.clone();
    while let Some(Ok(msg)) = receiver.next().await {
        match msg {
            Message::Text(text) => {
                if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                    handle_client_message(&session_for_recv, &client_id, client_msg);
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    send_task.abort();
}

fn handle_client_message(session: &CanvasSession, client_id: &str, msg: ClientMessage) {
    match msg {
        ClientMessage::PositionUpdate { card_id, x, y } => {
            session.push_event(hive_canvas::CanvasEvent::NodeUpdated {
                node_id: card_id,
                patch: hive_canvas::NodePatch {
                    content: None,
                    status: None,
                    x: Some(x),
                    y: Some(y),
                },
            });
        }
        ClientMessage::PromptSubmit { content, x, y } => {
            let node = hive_canvas::CanvasNode {
                id: uuid::Uuid::new_v4().to_string(),
                canvas_id: session.session_id.clone(),
                card_type: hive_canvas::CardType::Prompt,
                x,
                y,
                width: 280.0,
                height: 60.0,
                content: serde_json::json!({ "text": content }),
                status: hive_canvas::CardStatus::Active,
                created_by: client_id.to_string(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            };
            session.push_event(hive_canvas::CanvasEvent::NodeCreated { node, parent_edge: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_deserialization() {
        let json = r#"{"type":"position_update","card_id":"n1","x":10.0,"y":20.0}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::PositionUpdate { card_id, x, y } => {
                assert_eq!(card_id, "n1");
                assert_eq!(x, 10.0);
                assert_eq!(y, 20.0);
            }
            _ => panic!("wrong variant"),
        }

        let json = r#"{"type":"prompt_submit","content":"hello","x":1.0,"y":2.0}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::PromptSubmit { content, x, y } => {
                assert_eq!(content, "hello");
                assert_eq!(x, 1.0);
                assert_eq!(y, 2.0);
            }
            _ => panic!("wrong variant"),
        }
    }
}
