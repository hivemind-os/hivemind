//! Axum middleware that validates a `Bearer` token on incoming requests.
//!
//! Exempt routes (health-check, daemon status) are allowed through
//! without a token so that clients can bootstrap before they have read
//! the token from the OS keyring.

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};

use crate::AppState;

/// Routes that do **not** require authentication.
const EXEMPT_PATHS: &[&str] = &["/healthz", "/api/v1/daemon/status"];

/// Axum middleware: reject requests that lack a valid `Authorization: Bearer <token>` header.
pub async fn require_daemon_token(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Result<Response, StatusCode> {
    // Let exempt routes pass through unconditionally.
    let path = request.uri().path();
    if EXEMPT_PATHS.contains(&path) {
        return Ok(next.run(request).await);
    }

    let expected = &state.auth_token;

    let provided = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {
            Ok(next.run(request).await)
        }
        _ => Err(StatusCode::UNAUTHORIZED),
    }
}

/// Constant-time byte comparison to prevent timing side-channel attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exempt_paths_are_listed() {
        assert!(EXEMPT_PATHS.contains(&"/healthz"));
        assert!(EXEMPT_PATHS.contains(&"/api/v1/daemon/status"));
    }
}
