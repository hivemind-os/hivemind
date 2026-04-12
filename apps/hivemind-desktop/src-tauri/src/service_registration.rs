//! Per-user daemon service registration.
//!
//! On first launch of the desktop app, this module registers the hive-daemon
//! as a per-user background service so it starts automatically at login.
//!
//! Delegates to `hive_core::service_manager` for the actual OS-level work.

use tracing::warn;

/// Register the daemon as a per-user background service if not already
/// registered.  This is idempotent — calling it multiple times is safe.
pub fn ensure_daemon_service_registered() {
    if let Err(e) = hive_core::service_load() {
        warn!("failed to register daemon auto-start service: {e:#}");
    }
}
