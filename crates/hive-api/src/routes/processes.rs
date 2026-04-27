use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::{Deserialize, Serialize};

use crate::AppState;

#[derive(Serialize)]
pub(crate) struct ProcessListResponse {
    pub(crate) processes: Vec<hive_process::ProcessInfo>,
}

#[derive(Deserialize)]
pub(crate) struct ProcessStatusQuery {
    pub(crate) tail_lines: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct ProcessStatusResponse {
    pub(crate) info: hive_process::ProcessInfo,
    pub(crate) output: String,
}

#[derive(Deserialize)]
pub(crate) struct KillProcessRequest {
    pub(crate) signal: Option<String>,
}

/// List all background processes (with owner info).
pub(crate) async fn api_list_processes(State(state): State<AppState>) -> Json<ProcessListResponse> {
    let processes = state.chat.process_manager().list();
    Json(ProcessListResponse { processes })
}

/// List background processes owned by a specific session.
pub(crate) async fn api_list_session_processes(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Json<ProcessListResponse> {
    let processes = state.chat.process_manager().list_by_session(&session_id);
    Json(ProcessListResponse { processes })
}

/// Get status and recent output of a specific process.
pub(crate) async fn api_process_status(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Query(params): Query<ProcessStatusQuery>,
) -> Result<Json<ProcessStatusResponse>, (StatusCode, String)> {
    let (info, output) = state
        .chat
        .process_manager()
        .status(&process_id, params.tail_lines)
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(Json(ProcessStatusResponse { info, output }))
}

/// Kill a background process.
pub(crate) async fn api_kill_process(
    State(state): State<AppState>,
    Path(process_id): Path<String>,
    Json(body): Json<KillProcessRequest>,
) -> Result<Json<hive_process::ProcessInfo>, (StatusCode, String)> {
    let info = state
        .chat
        .process_manager()
        .kill(&process_id, body.signal.as_deref())
        .map_err(|e| (StatusCode::NOT_FOUND, e))?;
    Ok(Json(info))
}

/// SSE stream of process lifecycle events for a specific session.
pub(crate) async fn api_process_event_stream(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.chat.process_manager().subscribe();
    let stream = async_stream::stream! {
        // Send initial snapshot so late-joiners see current state.
        let snapshot = state.chat.process_manager().list_by_session(&session_id);
        if let Ok(json) = serde_json::to_string(&snapshot) {
            yield Ok(Event::default().event("snapshot").data(json));
        }
        loop {
            tokio::select! {
                biased;
                _ = state.shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            let matches = match &event {
                                hive_process::ProcessEvent::Spawned { session_id: sid, .. } => {
                                    sid.as_deref() == Some(session_id.as_str())
                                }
                                hive_process::ProcessEvent::Exited { session_id: sid, .. } => {
                                    sid.as_deref() == Some(session_id.as_str())
                                }
                                hive_process::ProcessEvent::Killed { session_id: sid, .. } => {
                                    sid.as_deref() == Some(session_id.as_str())
                                }
                            };
                            if matches {
                                if let Ok(json) = serde_json::to_string(&event) {
                                    yield Ok(Event::default().event("process").data(json));
                                }
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("process SSE for session {session_id} lagged {n} events");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}
