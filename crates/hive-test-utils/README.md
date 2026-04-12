# hive-test-utils

Testing utilities for integration tests in [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent.

This crate provides two core building blocks for test harnesses: a scriptable mock model provider and an ephemeral daemon that spins up a real HTTP server on a random port.

## MockProvider

A configurable mock that records calls and returns scripted responses. Built with a **builder pattern** for ergonomic test setup.

### Capabilities

| Method | Description |
|---|---|
| `MockProvider::new()` | Create a provider with a sensible default response |
| `.on_contains(needle, response)` | Return `response` when the prompt contains `needle` |
| `.default_response(response)` | Override the fallback response for unmatched prompts |
| `.with_latency(duration)` | Add artificial latency to every call |
| `.fail_after(n)` | Return `MockProviderError::ForcedFailure` after `n` successful calls |
| `.with_streaming(enabled, token_delay)` | Simulate token-by-token streaming with a per-token delay |
| `.invoke(prompt)` | Execute a request and get a `Result<MockResponse, MockProviderError>` |
| `.call_count()` | Number of calls made so far |
| `.calls()` | Full history of `MockCall` values (each carrying the prompt text) |

### Example

```rust
use hive_test_utils::MockProvider;
use std::time::Duration;

#[tokio::test]
async fn mock_provider_usage() {
    let provider = MockProvider::new()
        .on_contains("classify", "CONFIDENTIAL")
        .on_contains("summarize", "This is a summary.")
        .default_response("DEFAULT")
        .with_latency(Duration::from_millis(10));

    let res = provider.invoke("please classify this document").await.unwrap();
    assert_eq!(res.content, "CONFIDENTIAL");

    assert_eq!(provider.call_count(), 1);
    assert_eq!(provider.calls()[0].prompt, "please classify this document");
}
```

### Simulating failures

```rust
use hive_test_utils::{MockProvider, MockProviderError};

#[tokio::test]
async fn fails_after_two() {
    let provider = MockProvider::new().fail_after(2);

    assert!(provider.invoke("first").await.is_ok());
    assert!(provider.invoke("second").await.is_ok());
    assert_eq!(
        provider.invoke("third").await.unwrap_err(),
        MockProviderError::ForcedFailure { call_number: 3 },
    );
}
```

## TestDaemon

Spawns a fully functional HiveMind OS daemon bound to `127.0.0.1` on a random ephemeral port. Useful for end-to-end HTTP tests against the real API router.

- Automatically creates a temporary directory for audit logs and databases.
- Exposes `base_url` (e.g. `http://127.0.0.1:54321`) for issuing requests.
- Cleans up all resources on `stop()`.

### Example

```rust
use hive_test_utils::TestDaemon;

#[tokio::test]
async fn health_check() {
    let daemon = TestDaemon::spawn().await.expect("spawn daemon");

    let resp = reqwest::get(format!("{}/healthz", daemon.base_url))
        .await
        .expect("request");
    assert!(resp.status().is_success());

    daemon.stop().await.expect("graceful shutdown");
}
```

## Dependencies

### Workspace (internal)

- **hive-api** — Application state, router, and chat service used by `TestDaemon`.
- **hive-core** — Core types (`HiveMindConfig`, `AuditLogger`, `EventBus`).

### External

| Crate | Purpose |
|---|---|
| `tokio` | Async runtime and TCP listener |
| `reqwest` | HTTP client for integration tests |
| `tempfile` | Temporary directories for test isolation |
| `axum` | HTTP server used by `TestDaemon` |
| `thiserror` | Derive macro for `MockProviderError` |
| `serde` | Serialization support |
| `anyhow` | Error propagation in `TestDaemon` |
