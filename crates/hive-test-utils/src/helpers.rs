//! Polling helpers for backend integration tests.
//!
//! These async utilities wait for conditions in the running daemon (pending
//! questions, workflow status, etc.) with configurable timeouts, avoiding
//! brittle fixed sleeps.

use std::future::Future;
use std::time::Duration;

use anyhow::{bail, Result};

/// Poll `condition` every `poll_interval` until it returns `Some(T)`, or
/// fail after `timeout`.
pub async fn wait_for<T, F, Fut>(
    timeout: Duration,
    poll_interval: Duration,
    condition: F,
) -> Result<T>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let start = std::time::Instant::now();
    loop {
        if let Some(value) = condition().await {
            return Ok(value);
        }
        if start.elapsed() > timeout {
            bail!("wait_for timed out after {timeout:?}");
        }
        tokio::time::sleep(poll_interval).await;
    }
}

/// Poll `condition` every `poll_interval` until it returns `true`, or
/// fail after `timeout`.
pub async fn wait_until<F, Fut>(
    timeout: Duration,
    poll_interval: Duration,
    condition: F,
) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: Future<Output = bool>,
{
    wait_for(timeout, poll_interval, || async {
        if condition().await {
            Some(())
        } else {
            None
        }
    })
    .await
}

/// Default timeout for integration test polls.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default poll interval for integration test polls.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(250);
