//! Plugin health monitoring, crash detection, and auto-restart.
//!
//! Monitors running plugin processes and automatically restarts them
//! if they crash unexpectedly, with exponential backoff.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{error, info, warn};

/// Health state for a single plugin.
#[derive(Debug, Clone)]
pub struct PluginHealth {
    pub plugin_id: String,
    pub state: HealthState,
    pub consecutive_crashes: u32,
    pub last_crash: Option<Instant>,
    pub last_healthy: Option<Instant>,
    pub total_restarts: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthState {
    Healthy,
    Degraded { reason: String },
    Crashed { exit_code: Option<i32> },
    Backoff { until: Instant },
    Disabled,
}

/// Configuration for the health monitor.
#[derive(Debug, Clone)]
pub struct HealthConfig {
    /// How often to check plugin health.
    pub check_interval: Duration,
    /// Initial backoff delay after a crash.
    pub initial_backoff: Duration,
    /// Maximum backoff delay.
    pub max_backoff: Duration,
    /// Backoff multiplier.
    pub backoff_multiplier: f64,
    /// Max consecutive crashes before disabling auto-restart.
    pub max_consecutive_crashes: u32,
    /// Time window to reset the crash counter.
    pub crash_reset_window: Duration,
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            check_interval: Duration::from_secs(10),
            initial_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(300), // 5 minutes
            backoff_multiplier: 2.0,
            max_consecutive_crashes: 5,
            crash_reset_window: Duration::from_secs(600), // 10 min of stability resets counter
        }
    }
}

/// Tracks plugin health across all plugins.
pub struct HealthMonitor {
    config: HealthConfig,
    states: parking_lot::RwLock<HashMap<String, PluginHealth>>,
}

impl HealthMonitor {
    pub fn new(config: HealthConfig) -> Self {
        Self { config, states: parking_lot::RwLock::new(HashMap::new()) }
    }

    /// Register a plugin for health monitoring.
    pub fn register(&self, plugin_id: &str) {
        self.states.write().insert(
            plugin_id.to_string(),
            PluginHealth {
                plugin_id: plugin_id.to_string(),
                state: HealthState::Healthy,
                consecutive_crashes: 0,
                last_crash: None,
                last_healthy: Some(Instant::now()),
                total_restarts: 0,
            },
        );
    }

    /// Unregister a plugin from health monitoring.
    pub fn unregister(&self, plugin_id: &str) {
        self.states.write().remove(plugin_id);
    }

    /// Report that a plugin has crashed.
    pub fn report_crash(&self, plugin_id: &str, exit_code: Option<i32>) -> RestartDecision {
        let mut states = self.states.write();
        let health = states.entry(plugin_id.to_string()).or_insert_with(|| PluginHealth {
            plugin_id: plugin_id.to_string(),
            state: HealthState::Healthy,
            consecutive_crashes: 0,
            last_crash: None,
            last_healthy: None,
            total_restarts: 0,
        });

        let now = Instant::now();

        // Reset crash counter if plugin was stable for long enough
        if let Some(last_healthy) = health.last_healthy {
            if now.duration_since(last_healthy) > self.config.crash_reset_window {
                health.consecutive_crashes = 0;
            }
        }

        health.consecutive_crashes += 1;
        health.last_crash = Some(now);
        health.state = HealthState::Crashed { exit_code };

        if health.consecutive_crashes > self.config.max_consecutive_crashes {
            health.state = HealthState::Disabled;
            error!(
                plugin_id,
                crashes = health.consecutive_crashes,
                "Plugin exceeded max crashes, disabling auto-restart"
            );
            return RestartDecision::Disabled;
        }

        // Calculate backoff delay
        let backoff_secs = self.config.initial_backoff.as_secs_f64()
            * self.config.backoff_multiplier.powi((health.consecutive_crashes - 1) as i32);
        let backoff =
            Duration::from_secs_f64(backoff_secs.min(self.config.max_backoff.as_secs_f64()));

        health.state = HealthState::Backoff { until: now + backoff };
        health.total_restarts += 1;

        warn!(
            plugin_id,
            crash_count = health.consecutive_crashes,
            backoff_secs = backoff.as_secs(),
            "Plugin crashed, scheduling restart"
        );

        RestartDecision::RestartAfter(backoff)
    }

    /// Report that a plugin is healthy (call periodically or after successful operations).
    pub fn report_healthy(&self, plugin_id: &str) {
        let mut states = self.states.write();
        if let Some(health) = states.get_mut(plugin_id) {
            health.state = HealthState::Healthy;
            health.last_healthy = Some(Instant::now());
        }
    }

    /// Get the health state of a specific plugin.
    pub fn get_health(&self, plugin_id: &str) -> Option<PluginHealth> {
        self.states.read().get(plugin_id).cloned()
    }

    /// Get health summary for all monitored plugins.
    pub fn all_health(&self) -> Vec<PluginHealth> {
        self.states.read().values().cloned().collect()
    }

    /// Check if a plugin should be restarted now.
    pub fn should_restart(&self, plugin_id: &str) -> bool {
        let states = self.states.read();
        match states.get(plugin_id) {
            Some(health) => match &health.state {
                HealthState::Backoff { until } => Instant::now() >= *until,
                HealthState::Crashed { .. } => true,
                _ => false,
            },
            None => false,
        }
    }

    /// Reset a disabled plugin so it can be restarted manually.
    pub fn reset(&self, plugin_id: &str) {
        let mut states = self.states.write();
        if let Some(health) = states.get_mut(plugin_id) {
            health.consecutive_crashes = 0;
            health.state = HealthState::Healthy;
            info!(plugin_id, "Plugin health reset");
        }
    }
}

/// Decision about whether/when to restart a crashed plugin.
#[derive(Debug, Clone, PartialEq)]
pub enum RestartDecision {
    /// Restart after the given delay.
    RestartAfter(Duration),
    /// Too many crashes — don't restart automatically.
    Disabled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_health_lifecycle() {
        let monitor = HealthMonitor::new(HealthConfig::default());
        monitor.register("test-plugin");

        let health = monitor.get_health("test-plugin").unwrap();
        assert_eq!(health.state, HealthState::Healthy);

        // First crash → restart after initial backoff
        let decision = monitor.report_crash("test-plugin", Some(1));
        match decision {
            RestartDecision::RestartAfter(d) => {
                assert_eq!(d.as_secs(), 2); // initial_backoff
            }
            _ => panic!("Expected RestartAfter"),
        }

        let health = monitor.get_health("test-plugin").unwrap();
        assert_eq!(health.consecutive_crashes, 1);
        assert_eq!(health.total_restarts, 1);
    }

    #[test]
    fn test_exponential_backoff() {
        let monitor = HealthMonitor::new(HealthConfig::default());
        monitor.register("test-plugin");

        // First crash: 2s
        let d1 = monitor.report_crash("test-plugin", None);
        // Second crash: 4s
        let d2 = monitor.report_crash("test-plugin", None);
        // Third crash: 8s
        let d3 = monitor.report_crash("test-plugin", None);

        match (d1, d2, d3) {
            (
                RestartDecision::RestartAfter(a),
                RestartDecision::RestartAfter(b),
                RestartDecision::RestartAfter(c),
            ) => {
                assert_eq!(a.as_secs(), 2);
                assert_eq!(b.as_secs(), 4);
                assert_eq!(c.as_secs(), 8);
            }
            _ => panic!("Expected all RestartAfter"),
        }
    }

    #[test]
    fn test_max_crashes_disables() {
        let config = HealthConfig { max_consecutive_crashes: 3, ..Default::default() };
        let monitor = HealthMonitor::new(config);
        monitor.register("test-plugin");

        let _ = monitor.report_crash("test-plugin", None);
        let _ = monitor.report_crash("test-plugin", None);
        let _ = monitor.report_crash("test-plugin", None);
        let decision = monitor.report_crash("test-plugin", None);

        assert_eq!(decision, RestartDecision::Disabled);
    }

    #[test]
    fn test_manual_reset() {
        let config = HealthConfig { max_consecutive_crashes: 1, ..Default::default() };
        let monitor = HealthMonitor::new(config);
        monitor.register("test-plugin");

        let _ = monitor.report_crash("test-plugin", None);
        let _ = monitor.report_crash("test-plugin", None); // Disabled

        let health = monitor.get_health("test-plugin").unwrap();
        assert_eq!(health.state, HealthState::Disabled);

        // Manual reset
        monitor.reset("test-plugin");
        let health = monitor.get_health("test-plugin").unwrap();
        assert_eq!(health.state, HealthState::Healthy);
        assert_eq!(health.consecutive_crashes, 0);
    }

    #[test]
    fn test_should_restart() {
        let config =
            HealthConfig { initial_backoff: Duration::from_millis(10), ..Default::default() };
        let monitor = HealthMonitor::new(config);
        monitor.register("test-plugin");

        // Healthy plugin should not restart
        assert!(!monitor.should_restart("test-plugin"));

        // Crash and wait for backoff
        let _ = monitor.report_crash("test-plugin", None);
        std::thread::sleep(Duration::from_millis(20));
        assert!(monitor.should_restart("test-plugin"));
    }
}
