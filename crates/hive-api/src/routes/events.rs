use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

// ── Event Bus publish (for testing) ────────────────────────────────

#[derive(Deserialize)]
pub(crate) struct PublishEventRequest {
    pub topic: String,
    pub payload: Value,
}

pub(crate) async fn api_publish_event(
    State(state): State<AppState>,
    Json(req): Json<PublishEventRequest>,
) -> StatusCode {
    let _ = state.event_bus.publish(&req.topic, "api", req.payload);
    StatusCode::OK
}

// ── Event Bus SSE stream ────────────────────────────────────────────

/// Push-based SSE stream for ALL EventBus topics.
pub(crate) async fn api_event_bus_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.event_bus.subscribe_queued_bounded("", 10_000);
    let stream = async_stream::stream! {
        while let Some(envelope) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&envelope) {
                yield Ok(Event::default().event("event").data(json));
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}

#[derive(Deserialize)]
pub(crate) struct EventQueryParams {
    topic: Option<String>,
    since: Option<u128>,
    before_id: Option<i64>,
    after_id: Option<i64>,
    limit: Option<usize>,
}

pub(crate) async fn api_query_events(
    State(state): State<AppState>,
    Query(params): Query<EventQueryParams>,
) -> Result<Json<Vec<hive_core::StoredEvent>>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    Ok(Json(log.query_events(
        params.topic.as_deref(),
        params.since,
        params.before_id,
        params.after_id,
        params.limit,
    )))
}

#[derive(Deserialize)]
pub(crate) struct PruneParams {
    before: u128,
}

pub(crate) async fn api_prune_events(
    State(state): State<AppState>,
    Query(params): Query<PruneParams>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    let deleted = log.prune_before(params.before);
    Ok(Json(json!({ "deleted": deleted })))
}

#[derive(Deserialize)]
pub(crate) struct StartRecordingRequest {
    name: Option<String>,
    topic_filter: Option<String>,
}

pub(crate) async fn api_start_recording(
    State(state): State<AppState>,
    Json(body): Json<StartRecordingRequest>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    let id = log.start_recording(body.name.as_deref(), body.topic_filter.as_deref());
    Ok(Json(json!({ "recording_id": id })))
}

pub(crate) async fn api_stop_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> Result<Json<hive_core::RecordingSummary>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    log.stop_recording(&recording_id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "recording not found or already stopped".to_string()))
}

pub(crate) async fn api_list_recordings(
    State(state): State<AppState>,
) -> Result<Json<Vec<hive_core::RecordingSummary>>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    Ok(Json(log.list_recordings()))
}

pub(crate) async fn api_get_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> Result<Json<hive_core::Recording>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    log.get_recording(&recording_id)
        .map(Json)
        .ok_or((StatusCode::NOT_FOUND, "recording not found".to_string()))
}

pub(crate) async fn api_delete_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;
    if log.delete_recording(&recording_id) {
        Ok(Json(json!({ "deleted": true })))
    } else {
        Err((StatusCode::NOT_FOUND, "recording not found".to_string()))
    }
}

#[derive(Deserialize)]
pub(crate) struct ExportParams {
    format: Option<String>,
}

pub(crate) async fn api_export_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<String>,
    Query(params): Query<ExportParams>,
) -> Result<axum::response::Response, (StatusCode, String)> {
    let log = state
        .event_log
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "event log not available".to_string()))?;

    let format = params.format.as_deref().unwrap_or("json");
    match format {
        "rust_test" => {
            let scaffold = log
                .export_test_scaffold(&recording_id)
                .ok_or((StatusCode::NOT_FOUND, "recording not found".to_string()))?;
            Ok(axum::response::Response::builder()
                .header("content-type", "text/x-rust")
                .header(
                    "content-disposition",
                    format!("attachment; filename=\"{recording_id}.rs\""),
                )
                .body(axum::body::Body::from(scaffold))
                .unwrap())
        }
        _ => {
            let json_str = log
                .export_json(&recording_id)
                .ok_or((StatusCode::NOT_FOUND, "recording not found".to_string()))?;
            Ok(axum::response::Response::builder()
                .header("content-type", "application/json")
                .header(
                    "content-disposition",
                    format!("attachment; filename=\"{recording_id}.json\""),
                )
                .body(axum::body::Body::from(json_str))
                .unwrap())
        }
    }
}
