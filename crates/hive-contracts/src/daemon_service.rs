use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Category for grouping daemon services in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceCategory {
    Core,
    Connector,
    Mcp,
    Agents,
    Inference,
}

impl std::fmt::Display for ServiceCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Core => write!(f, "Core"),
            Self::Connector => write!(f, "Connector"),
            Self::Mcp => write!(f, "MCP"),
            Self::Agents => write!(f, "Agents"),
            Self::Inference => write!(f, "Inference"),
        }
    }
}

/// Runtime status of a daemon service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceStatus {
    Running,
    Stopped,
    Starting,
    Stopping,
    Error,
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Running => write!(f, "Running"),
            Self::Stopped => write!(f, "Stopped"),
            Self::Starting => write!(f, "Starting"),
            Self::Stopping => write!(f, "Stopping"),
            Self::Error => write!(f, "Error"),
        }
    }
}

/// Serialisable snapshot of a service's current state, returned by the API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceSnapshot {
    pub id: String,
    pub display_name: String,
    pub category: ServiceCategory,
    pub status: ServiceStatus,
    pub last_error: Option<String>,
}

/// Lifecycle trait for daemon background services.
///
/// Every background service in the daemon implements this trait so the
/// service registry can start, stop, restart, and health-check them
/// uniformly.
#[async_trait]
pub trait DaemonService: Send + Sync {
    /// Unique service identifier (e.g. `"scheduler"`, `"mcp:my-server"`).
    fn service_id(&self) -> &str;

    /// Human-readable display name.
    fn display_name(&self) -> String;

    /// Category for UI grouping.
    fn category(&self) -> ServiceCategory;

    /// Current runtime status.
    fn status(&self) -> ServiceStatus;

    /// Start the service's background work.
    async fn start(&self) -> anyhow::Result<()>;

    /// Gracefully stop the service.
    async fn stop(&self) -> anyhow::Result<()>;

    /// Restart the service (default: stop then start).
    async fn restart(&self) -> anyhow::Result<()> {
        self.stop().await?;
        self.start().await
    }

    /// Last error message, if the service is in an error state.
    fn last_error(&self) -> Option<String> {
        None
    }

    /// Build a serialisable snapshot of the service.
    fn snapshot(&self) -> ServiceSnapshot {
        ServiceSnapshot {
            id: self.service_id().to_string(),
            display_name: self.display_name(),
            category: self.category(),
            status: self.status(),
            last_error: self.last_error(),
        }
    }
}
