use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use std::convert::Infallible;

use crate::AppState;
use hive_node_env::NodeEnvStatus;

/// GET /api/v1/node/status — return current managed Node.js environment status.
pub(crate) async fn node_status(State(state): State<AppState>) -> Json<NodeEnvStatus> {
    Json(state.node_env.status().await)
}

/// GET /api/v1/node/status/stream — SSE stream of Node.js env status changes.
pub(crate) async fn node_status_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.node_env.subscribe();
    let current = state.node_env.status().await;
    let stream = async_stream::stream! {
        // Send current status as initial event.
        yield Ok(Event::default().data(
            serde_json::to_string(&current).unwrap_or_default()
        ));
        loop {
            match rx.recv().await {
                Ok(status) => {
                    yield Ok(Event::default().data(
                        serde_json::to_string(&status).unwrap_or_default()
                    ));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// POST /api/v1/node/reinstall — force re-download the managed Node.js.
pub(crate) async fn node_reinstall(
    State(state): State<AppState>,
) -> Result<Json<NodeEnvStatus>, (StatusCode, String)> {
    match state.node_env.reinstall().await {
        Ok(dist_dir) => {
            // Update shared shell env vars so the process tool picks up the new PATH.
            if let Some(vars) = state.node_env.shell_env_vars().await {
                crate::services::merge_runtime_env(&state.shell_env, vars);
            }
            tracing::info!(path = %dist_dir.display(), "Node.js environment reinstalled");
            Ok(Json(state.node_env.status().await))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("reinstall failed: {e}"))),
    }
}
