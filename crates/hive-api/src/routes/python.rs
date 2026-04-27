use axum::{
    extract::State,
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use std::convert::Infallible;

use crate::AppState;
use hive_python_env::PythonEnvStatus;

/// GET /api/v1/python/status — return current managed Python environment status.
pub(crate) async fn python_status(State(state): State<AppState>) -> Json<PythonEnvStatus> {
    Json(state.python_env.status().await)
}

/// GET /api/v1/python/status/stream — SSE stream of Python env status changes.
pub(crate) async fn python_status_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.python_env.subscribe();
    let current = state.python_env.status().await;
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        // Send current status as initial event.
        yield Ok(Event::default().data(
            serde_json::to_string(&current).unwrap_or_default()
        ));
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(status) => {
                            yield Ok(Event::default().data(
                                serde_json::to_string(&status).unwrap_or_default()
                            ));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new()
            .interval(std::time::Duration::from_secs(15))
            .text("keep-alive"),
    )
}

/// POST /api/v1/python/reinstall — force-rebuild the managed Python environment.
pub(crate) async fn python_reinstall(
    State(state): State<AppState>,
) -> Result<Json<PythonEnvStatus>, (StatusCode, String)> {
    let shell_env = state.shell_env.clone();
    let python_env = state.python_env.clone();

    match python_env.reinstall().await {
        Ok(info) => {
            // Update shared shell env vars.
            if let Some(vars) = python_env.shell_env_vars(None).await {
                crate::services::merge_runtime_env(&shell_env, vars);
            }
            tracing::info!(venv = %info.venv_path.display(), "Python environment reinstalled");
            Ok(Json(python_env.status().await))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("reinstall failed: {e}"))),
    }
}
