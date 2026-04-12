use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;

use crate::{shared_api_client, AppState};

/// Keyring key used to store the GitHub OAuth token.
const GITHUB_TOKEN_KEY: &str = "github:oauth-token";

/// Start a GitHub device flow — returns the user code and verification URI.
pub(crate) async fn github_start_device_flow(State(state): State<AppState>) -> impl IntoResponse {
    let client = shared_api_client();
    match crate::provider_auth::github_device_code_request(client).await {
        Ok(resp) => {
            state.pending_device_codes.lock().insert(resp.device_code.clone(), resp.clone());
            (
                StatusCode::OK,
                Json(json!({
                    "device_code": resp.device_code,
                    "user_code": resp.user_code,
                    "verification_uri": resp.verification_uri,
                    "expires_in": resp.expires_in,
                    "interval": resp.interval,
                })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response(),
    }
}

/// Poll for a GitHub token using a device code.
pub(crate) async fn github_poll_token(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let device_code = body["device_code"].as_str().unwrap_or_default();
    let client = shared_api_client();
    match crate::provider_auth::github_poll_for_token(client, device_code).await {
        Ok(resp) => {
            if let Some(token) = &resp.access_token {
                (
                    StatusCode::OK,
                    Json(json!({
                        "status": "complete",
                        "access_token": token,
                    })),
                )
                    .into_response()
            } else if resp.error.as_deref() == Some("authorization_pending") {
                (StatusCode::OK, Json(json!({ "status": "pending" }))).into_response()
            } else {
                let error = resp
                    .error_description
                    .or(resp.error)
                    .unwrap_or_else(|| "unknown error".to_string());
                (
                    StatusCode::OK,
                    Json(json!({
                        "status": "failed",
                        "error": error,
                    })),
                )
                    .into_response()
            }
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": e.to_string() })))
            .into_response(),
    }
}

/// Save a GitHub token to the OS keyring.
pub(crate) async fn github_save_token(
    State(_state): State<AppState>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let provider_id = body["provider_id"].as_str().unwrap_or("github-copilot");
    let token = match body["token"].as_str() {
        Some(t) => t,
        None => {
            return (StatusCode::BAD_REQUEST, Json(json!({ "error": "missing 'token' field" })))
                .into_response()
        }
    };

    let persisted = hive_core::secret_store::save(GITHUB_TOKEN_KEY, token);

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "provider_id": provider_id,
            "persisted": persisted,
        })),
    )
        .into_response()
}

/// Check whether a saved GitHub token exists and is valid.
pub(crate) async fn github_auth_status(State(_state): State<AppState>) -> impl IntoResponse {
    let token = hive_core::secret_store::load(GITHUB_TOKEN_KEY)
        .filter(|t| !t.is_empty())
        .map(|t| t.trim().to_string());

    let token = match token {
        Some(t) => t,
        None => {
            return (StatusCode::OK, Json(json!({ "authenticated": false }))).into_response();
        }
    };

    let client = shared_api_client();
    match client
        .get("https://api.github.com/user")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "hivemind-desktop")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let username = body["login"].as_str().unwrap_or("").to_string();
            (StatusCode::OK, Json(json!({ "authenticated": true, "username": username })))
                .into_response()
        }
        Ok(resp) if resp.status() == reqwest::StatusCode::UNAUTHORIZED => {
            // Token is invalid — clean up.
            hive_core::secret_store::delete(GITHUB_TOKEN_KEY);
            (StatusCode::OK, Json(json!({ "authenticated": false }))).into_response()
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            (
                StatusCode::OK,
                Json(json!({ "authenticated": false, "error": format!("GitHub API returned {status}") })),
            )
                .into_response()
        }
        Err(e) => (StatusCode::OK, Json(json!({ "authenticated": false, "error": e.to_string() })))
            .into_response(),
    }
}

/// Fetch the list of models available through the GitHub Copilot API.
pub(crate) async fn github_list_models() -> impl IntoResponse {
    let token = match hive_core::secret_store::load(GITHUB_TOKEN_KEY).filter(|t| !t.is_empty()) {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "error": "not authenticated – no GitHub token" })),
            )
                .into_response();
        }
    };

    let client = shared_api_client();
    match client
        .get("https://api.githubcopilot.com/models")
        .header("Authorization", format!("Bearer {token}"))
        .header("User-Agent", "hivemind-desktop")
        .header("Accept", "application/json")
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or(json!([]));
            (StatusCode::OK, Json(body)).into_response()
        }
        Ok(resp) => {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": format!("Copilot API returned {status}"), "details": text })),
            )
                .into_response()
        }
        Err(e) => {
            (StatusCode::BAD_GATEWAY, Json(json!({ "error": e.to_string() }))).into_response()
        }
    }
}

/// Remove the saved GitHub token (disconnect).
pub(crate) async fn github_disconnect(State(_state): State<AppState>) -> impl IntoResponse {
    hive_core::secret_store::delete(GITHUB_TOKEN_KEY);
    (StatusCode::OK, Json(json!({ "ok": true }))).into_response()
}
