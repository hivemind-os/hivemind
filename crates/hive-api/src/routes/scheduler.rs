use crate::{scheduler_error, AppState};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::sse::{Event, Sse},
    Json,
};
use hive_scheduler::{
    CreateTaskRequest, ListTasksFilter, ScheduledTask, TaskRun, UpdateTaskRequest,
};

pub(crate) async fn list_scheduler_tasks(
    State(state): State<AppState>,
    Query(filter): Query<ListTasksFilter>,
) -> Result<Json<Vec<ScheduledTask>>, (StatusCode, String)> {
    state.scheduler.list_tasks_filtered(&filter).map(Json).map_err(scheduler_error)
}

pub(crate) async fn create_scheduler_task(
    State(state): State<AppState>,
    Json(request): Json<CreateTaskRequest>,
) -> Result<(StatusCode, Json<ScheduledTask>), (StatusCode, String)> {
    state
        .scheduler
        .create_task(request)
        .map(|t| (StatusCode::CREATED, Json(t)))
        .map_err(scheduler_error)
}

pub(crate) async fn get_scheduler_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<ScheduledTask>, (StatusCode, String)> {
    state.scheduler.get_task(&task_id).map(Json).map_err(scheduler_error)
}

pub(crate) async fn update_scheduler_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
    Json(request): Json<UpdateTaskRequest>,
) -> Result<Json<ScheduledTask>, (StatusCode, String)> {
    state.scheduler.update_task(&task_id, request).map(Json).map_err(scheduler_error)
}

pub(crate) async fn cancel_scheduler_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<ScheduledTask>, (StatusCode, String)> {
    state.scheduler.cancel_task(&task_id).map(Json).map_err(scheduler_error)
}

pub(crate) async fn delete_scheduler_task(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state.scheduler.delete_task(&task_id).map(|_| StatusCode::NO_CONTENT).map_err(scheduler_error)
}

pub(crate) async fn list_scheduler_task_runs(
    State(state): State<AppState>,
    Path(task_id): Path<String>,
) -> Result<Json<Vec<TaskRun>>, (StatusCode, String)> {
    state.scheduler.list_task_runs(&task_id).map(Json).map_err(scheduler_error)
}

pub(crate) async fn api_scheduler_event_stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let mut rx = state.event_bus.subscribe_queued_bounded("scheduler", 10_000);
    let shutdown = state.shutdown.clone();
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                biased;
                _ = shutdown.cancelled() => break,
                msg = rx.recv() => {
                    match msg {
                        Some(envelope) => {
                            if let Ok(json) = serde_json::to_string(&envelope) {
                                yield Ok(Event::default().event("scheduler").data(json));
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    };
    Sse::new(stream).keep_alive(
        axum::response::sse::KeepAlive::new().interval(std::time::Duration::from_secs(15)),
    )
}
