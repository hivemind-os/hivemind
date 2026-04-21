//! Plugin permission enforcement and resource limits.
//!
//! Validates that plugin host-API calls comply with their declared permissions.
//! This is trust-based enforcement — the plugin declared permissions in its
//! manifest and we verify calls match those declarations.

use crate::manifest::HivemindMeta;
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Permissions a plugin can declare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    /// Network access to a specific domain: `network:<domain>`
    Network(String),
    /// Secret storage read: `secrets:read`
    SecretsRead,
    /// Secret storage write: `secrets:write`
    SecretsWrite,
    /// Background loop: `loop:background`
    LoopBackground,
    /// File system access: `filesystem:<path>`
    Filesystem(String),
    /// Unknown permission (future-proof)
    Unknown(String),
}

impl Permission {
    pub fn parse(s: &str) -> Self {
        if let Some(domain) = s.strip_prefix("network:") {
            Self::Network(domain.to_string())
        } else if s == "secrets:read" {
            Self::SecretsRead
        } else if s == "secrets:write" {
            Self::SecretsWrite
        } else if s == "loop:background" {
            Self::LoopBackground
        } else if let Some(path) = s.strip_prefix("filesystem:") {
            Self::Filesystem(path.to_string())
        } else {
            Self::Unknown(s.to_string())
        }
    }
}

/// Result of a permission check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionCheckResult {
    pub allowed: bool,
    pub reason: Option<String>,
}

/// Plugin sandbox enforcing declared permissions and resource limits.
pub struct PluginSandbox {
    plugin_id: String,
    permissions: Vec<Permission>,
    resource_limits: ResourceLimits,
    metrics: parking_lot::Mutex<PluginMetrics>,
}

/// Configurable resource limits per plugin.
#[derive(Debug, Clone)]
pub struct ResourceLimits {
    /// Maximum pending requests (prevents runaway plugins).
    pub max_pending_requests: usize,
    /// Maximum message emission rate (per minute).
    pub max_messages_per_minute: u32,
    /// Maximum event emission rate (per minute).
    pub max_events_per_minute: u32,
    /// Request timeout.
    pub request_timeout: Duration,
    /// Maximum payload size in bytes.
    pub max_payload_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_pending_requests: 100,
            max_messages_per_minute: 600,
            max_events_per_minute: 300,
            request_timeout: Duration::from_secs(30),
            max_payload_bytes: 10 * 1024 * 1024, // 10 MB
        }
    }
}

/// Runtime metrics tracked per plugin for rate limiting.
#[derive(Debug, Default)]
struct PluginMetrics {
    messages_emitted: RateCounter,
    events_emitted: RateCounter,
    errors: u64,
    last_activity: Option<Instant>,
}

/// Sliding-window rate counter.
#[derive(Debug)]
struct RateCounter {
    window: Duration,
    timestamps: Vec<Instant>,
}

impl Default for RateCounter {
    fn default() -> Self {
        Self { window: Duration::from_secs(60), timestamps: Vec::new() }
    }
}

impl RateCounter {
    fn record(&mut self) {
        let now = Instant::now();
        self.timestamps.push(now);
        self.prune(now);
    }

    fn count(&mut self) -> usize {
        self.prune(Instant::now());
        self.timestamps.len()
    }

    fn prune(&mut self, now: Instant) {
        self.timestamps.retain(|t| now.duration_since(*t) < self.window);
    }
}

impl PluginSandbox {
    pub fn new(plugin_id: String, manifest: &HivemindMeta) -> Self {
        let permissions = manifest.permissions.iter().map(|s| Permission::parse(s)).collect();

        Self {
            plugin_id,
            permissions,
            resource_limits: ResourceLimits::default(),
            metrics: parking_lot::Mutex::new(PluginMetrics::default()),
        }
    }

    pub fn with_limits(mut self, limits: ResourceLimits) -> Self {
        self.resource_limits = limits;
        self
    }

    /// Check if a host API call is permitted.
    pub fn check_host_call(
        &self,
        method: &str,
        _params: &serde_json::Value,
    ) -> PermissionCheckResult {
        use crate::protocol::host_methods::*;

        match method {
            SECRET_GET | SECRET_HAS => {
                if !self.has_permission_kind(|p| matches!(p, Permission::SecretsRead)) {
                    return PermissionCheckResult {
                        allowed: false,
                        reason: Some(format!(
                            "Plugin '{}' missing permission: secrets:read",
                            self.plugin_id
                        )),
                    };
                }
            }
            SECRET_SET | SECRET_DELETE => {
                if !self.has_permission_kind(|p| matches!(p, Permission::SecretsWrite)) {
                    return PermissionCheckResult {
                        allowed: false,
                        reason: Some(format!(
                            "Plugin '{}' missing permission: secrets:write",
                            self.plugin_id
                        )),
                    };
                }
            }
            _ => {}
        }

        PermissionCheckResult { allowed: true, reason: None }
    }

    /// Check if the plugin can start a background loop.
    pub fn can_start_loop(&self) -> bool {
        self.has_permission_kind(|p| matches!(p, Permission::LoopBackground))
    }

    /// Check message emission rate limit. Returns Err with reason if exceeded.
    pub fn check_message_rate(&self) -> Result<(), String> {
        let mut metrics = self.metrics.lock();
        metrics.messages_emitted.record();
        metrics.last_activity = Some(Instant::now());

        let count = metrics.messages_emitted.count();
        if count > self.resource_limits.max_messages_per_minute as usize {
            metrics.errors += 1;
            return Err(format!(
                "Plugin '{}' exceeded message rate limit: {}/{} per minute",
                self.plugin_id, count, self.resource_limits.max_messages_per_minute
            ));
        }
        Ok(())
    }

    /// Check event emission rate limit. Returns Err with reason if exceeded.
    pub fn check_event_rate(&self) -> Result<(), String> {
        let mut metrics = self.metrics.lock();
        metrics.events_emitted.record();
        metrics.last_activity = Some(Instant::now());

        let count = metrics.events_emitted.count();
        if count > self.resource_limits.max_events_per_minute as usize {
            metrics.errors += 1;
            return Err(format!(
                "Plugin '{}' exceeded event rate limit: {}/{} per minute",
                self.plugin_id, count, self.resource_limits.max_events_per_minute
            ));
        }
        Ok(())
    }

    /// Check if a payload exceeds the size limit.
    pub fn check_payload_size(&self, size: usize) -> Result<(), String> {
        if size > self.resource_limits.max_payload_bytes {
            return Err(format!(
                "Plugin '{}' payload too large: {} bytes (max {})",
                self.plugin_id, size, self.resource_limits.max_payload_bytes
            ));
        }
        Ok(())
    }

    /// Get the request timeout for this plugin.
    pub fn request_timeout(&self) -> Duration {
        self.resource_limits.request_timeout
    }

    /// Get accumulated error count.
    pub fn error_count(&self) -> u64 {
        self.metrics.lock().errors
    }

    fn has_permission_kind<F: Fn(&Permission) -> bool>(&self, predicate: F) -> bool {
        self.permissions.iter().any(|p| predicate(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_manifest(permissions: Vec<&str>) -> HivemindMeta {
        HivemindMeta {
            plugin_type: "connector".into(),
            display_name: "Test".into(),
            description: "Test plugin".into(),
            icon: None,
            categories: vec![],
            services: vec![],
            permissions: permissions.into_iter().map(String::from).collect(),
            min_host_version: None,
        }
    }

    #[test]
    fn test_permission_parsing() {
        assert_eq!(
            Permission::parse("network:api.example.com"),
            Permission::Network("api.example.com".into())
        );
        assert_eq!(Permission::parse("secrets:read"), Permission::SecretsRead);
        assert_eq!(Permission::parse("secrets:write"), Permission::SecretsWrite);
        assert_eq!(Permission::parse("loop:background"), Permission::LoopBackground);
        assert_eq!(Permission::parse("unknown:thing"), Permission::Unknown("unknown:thing".into()));
    }

    #[test]
    fn test_secret_permission_enforcement() {
        let manifest = make_manifest(vec!["secrets:read"]);
        let sandbox = PluginSandbox::new("test".into(), &manifest);

        // Read is allowed
        let result = sandbox.check_host_call("host/secretGet", &serde_json::json!({"key": "test"}));
        assert!(result.allowed);

        // Write is denied
        let result = sandbox
            .check_host_call("host/secretSet", &serde_json::json!({"key": "test", "value": "val"}));
        assert!(!result.allowed);
        assert!(result.reason.unwrap().contains("secrets:write"));
    }

    #[test]
    fn test_loop_permission() {
        let manifest = make_manifest(vec![]);
        let sandbox = PluginSandbox::new("test".into(), &manifest);
        assert!(!sandbox.can_start_loop());

        let manifest = make_manifest(vec!["loop:background"]);
        let sandbox = PluginSandbox::new("test".into(), &manifest);
        assert!(sandbox.can_start_loop());
    }

    #[test]
    fn test_message_rate_limit() {
        let manifest = make_manifest(vec![]);
        let limits = ResourceLimits { max_messages_per_minute: 5, ..Default::default() };
        let sandbox = PluginSandbox::new("test".into(), &manifest).with_limits(limits);

        for _ in 0..5 {
            assert!(sandbox.check_message_rate().is_ok());
        }

        // 6th should exceed limit
        assert!(sandbox.check_message_rate().is_err());
        assert_eq!(sandbox.error_count(), 1);
    }

    #[test]
    fn test_payload_size_limit() {
        let manifest = make_manifest(vec![]);
        let limits = ResourceLimits { max_payload_bytes: 1024, ..Default::default() };
        let sandbox = PluginSandbox::new("test".into(), &manifest).with_limits(limits);

        assert!(sandbox.check_payload_size(512).is_ok());
        assert!(sandbox.check_payload_size(2048).is_err());
    }
}
