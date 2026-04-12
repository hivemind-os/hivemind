use axum::{extract::Path, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

/// PUT /api/v1/secrets/:key — store a secret in the OS keyring.
pub(crate) async fn api_save_secret(
    Path(key): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl axum::response::IntoResponse {
    let value = match body.get("value").and_then(|v| v.as_str()) {
        Some(v) => v,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "missing `value` string in request body" })),
            )
                .into_response()
        }
    };

    let ok = hive_core::secret_store::save(&key, value);
    if ok {
        Json(json!({ "saved": true })).into_response()
    } else {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to persist secret to OS keyring" })),
        )
            .into_response()
    }
}

/// GET /api/v1/secrets/:key — load a secret from the OS keyring.
pub(crate) async fn api_load_secret(Path(key): Path<String>) -> Json<serde_json::Value> {
    match hive_core::secret_store::load(&key) {
        Some(value) => Json(json!({ "value": value })),
        None => Json(json!({ "value": null })),
    }
}

/// DELETE /api/v1/secrets/:key — remove a secret from the OS keyring.
pub(crate) async fn api_delete_secret(Path(key): Path<String>) -> Json<serde_json::Value> {
    hive_core::secret_store::delete(&key);
    Json(json!({ "deleted": true }))
}
