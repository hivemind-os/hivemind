pub mod helpers;
pub mod mock_connector;
pub mod scripted_provider;

pub use helpers::{wait_for, wait_until, DEFAULT_POLL_INTERVAL, DEFAULT_TIMEOUT};
pub use mock_connector::MockConnector;
pub use scripted_provider::{RecordedCall, ScriptedProvider};

use anyhow::{Context, Result};
use hive_api::{
    build_router, canvas_ws::CanvasSessionRegistry, chat, AppState, ChatRuntimeConfig, ChatService,
};
use hive_core::{AuditLogger, EventBus, HiveMindConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use thiserror::Error;
use tokio::net::TcpListener;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockCall {
    pub prompt: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockResponse {
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct MockProvider {
    state: Arc<Mutex<MockProviderState>>,
}

#[derive(Debug)]
struct MockProviderState {
    rules: Vec<MockRule>,
    default_response: String,
    latency: Duration,
    fail_after: Option<usize>,
    streaming: Option<Duration>,
    call_count: usize,
    calls: Vec<MockCall>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MockRule {
    needle: String,
    response: String,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MockProviderError {
    #[error("mock provider forced a failure on call {call_number}")]
    ForcedFailure { call_number: usize },
}

impl MockProvider {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(MockProviderState {
                rules: Vec::new(),
                default_response: "I'm a mock assistant. How can I help?".to_string(),
                latency: Duration::ZERO,
                fail_after: None,
                streaming: None,
                call_count: 0,
                calls: Vec::new(),
            })),
        }
    }

    pub fn on_contains(self, needle: impl Into<String>, response: impl Into<String>) -> Self {
        self.state
            .lock()
            .expect("mock provider mutex poisoned")
            .rules
            .push(MockRule { needle: needle.into(), response: response.into() });
        self
    }

    pub fn default_response(self, response: impl Into<String>) -> Self {
        self.state.lock().expect("mock provider mutex poisoned").default_response = response.into();
        self
    }

    pub fn with_latency(self, latency: Duration) -> Self {
        self.state.lock().expect("mock provider mutex poisoned").latency = latency;
        self
    }

    pub fn fail_after(self, successful_calls: usize) -> Self {
        self.state.lock().expect("mock provider mutex poisoned").fail_after =
            Some(successful_calls);
        self
    }

    pub fn with_streaming(self, enabled: bool, token_delay: Duration) -> Self {
        self.state.lock().expect("mock provider mutex poisoned").streaming =
            enabled.then_some(token_delay);
        self
    }

    pub async fn invoke(
        &self,
        prompt: &str,
    ) -> std::result::Result<MockResponse, MockProviderError> {
        let (response, latency, streaming, call_number, fail_after) = {
            let mut state = self.state.lock().expect("mock provider mutex poisoned");
            state.call_count += 1;
            state.calls.push(MockCall { prompt: prompt.to_string() });

            let response = state
                .rules
                .iter()
                .find(|rule| prompt.contains(&rule.needle))
                .map(|rule| rule.response.clone())
                .unwrap_or_else(|| state.default_response.clone());

            (response, state.latency, state.streaming, state.call_count, state.fail_after)
        };

        if let Some(successful_calls) = fail_after {
            if call_number > successful_calls {
                return Err(MockProviderError::ForcedFailure { call_number });
            }
        }

        if !latency.is_zero() {
            tokio::time::sleep(latency).await;
        }

        if let Some(token_delay) = streaming {
            for _ in response.split_whitespace() {
                tokio::time::sleep(token_delay).await;
            }
        }

        Ok(MockResponse { content: response })
    }

    pub fn call_count(&self) -> usize {
        self.state.lock().expect("mock provider mutex poisoned").call_count
    }

    pub fn calls(&self) -> Vec<MockCall> {
        self.state.lock().expect("mock provider mutex poisoned").calls.clone()
    }
}

impl Default for MockProvider {
    fn default() -> Self {
        Self::new()
    }
}

pub struct TestDaemon {
    pub base_url: String,
    pub event_bus: EventBus,
    shutdown: Arc<Notify>,
    handle: JoinHandle<Result<()>>,
    tempdir: TempDir,
}

impl TestDaemon {
    /// Returns the path to the temporary directory backing this daemon.
    ///
    /// Useful for creating workspace files before starting a chat session.
    pub fn tempdir_path(&self) -> &std::path::Path {
        self.tempdir.path()
    }
}

impl TestDaemon {
    pub async fn spawn() -> Result<Self> {
        TestDaemonBuilder::new().spawn().await
    }

    /// Create a builder for fine-grained configuration.
    pub fn builder() -> TestDaemonBuilder {
        TestDaemonBuilder::new()
    }

    pub async fn stop(self) -> Result<()> {
        self.shutdown.notify_waiters();
        self.handle.await.context("failed to join the test daemon task")??;
        Ok(())
    }
}

/// Builder for configuring a [`TestDaemon`] with custom providers, personas,
/// and workflows.
pub struct TestDaemonBuilder {
    model_router: Option<Arc<hive_model::ModelRouter>>,
    personas: Vec<hive_contracts::config::Persona>,
    connector_registry: Option<Arc<hive_connectors::ConnectorRegistry>>,
    connector_service: Option<Arc<dyn hive_connectors::ConnectorServiceHandle>>,
}

impl Default for TestDaemonBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TestDaemonBuilder {
    pub fn new() -> Self {
        Self {
            model_router: None,
            personas: Vec::new(),
            connector_registry: None,
            connector_service: None,
        }
    }

    /// Use a custom `ModelRouter` (e.g. with a [`ScriptedProvider`] registered).
    pub fn with_model_router(mut self, router: Arc<hive_model::ModelRouter>) -> Self {
        self.model_router = Some(router);
        self
    }

    /// Register personas on the daemon.
    pub fn with_personas(mut self, personas: Vec<hive_contracts::config::Persona>) -> Self {
        self.personas = personas;
        self
    }

    /// Use a pre-built connector registry (e.g. with [`MockConnector`] registered).
    pub fn with_connector_registry(
        mut self,
        registry: Arc<hive_connectors::ConnectorRegistry>,
    ) -> Self {
        self.connector_registry = Some(registry);
        self
    }

    /// Use a pre-built connector service for data-classification testing.
    pub fn with_connector_service(
        mut self,
        service: Arc<dyn hive_connectors::ConnectorServiceHandle>,
    ) -> Self {
        self.connector_service = Some(service);
        self
    }

    /// Build and start the test daemon.
    pub async fn spawn(self) -> Result<TestDaemon> {
        let tempdir = tempfile::tempdir().context("failed to create temp dir")?;
        let listener =
            TcpListener::bind("127.0.0.1:0").await.context("failed to bind an ephemeral port")?;
        let address =
            listener.local_addr().context("failed to determine the ephemeral listener address")?;

        let mut config = HiveMindConfig::default();
        config.api.bind = address.to_string();

        // Merge builder personas into config.
        if !self.personas.is_empty() {
            config.personas = self.personas;
        }

        let audit = AuditLogger::new(tempdir.path().join("audit.log"))
            .context("failed to create test audit log")?;
        let event_bus = EventBus::new(32);
        let shutdown = Arc::new(Notify::new());

        // Use the provided model router or build the default one.
        let model_router = match self.model_router {
            Some(router) => router,
            None => chat::build_model_router_from_config(&config, None, None)
                .expect("validated hivemind config should produce a model router"),
        };

        let connector_registry = self.connector_registry;
        let connector_service = self.connector_service;

        let chat = Arc::new(ChatService::with_model_router(
            audit.clone(),
            event_bus.clone(),
            ChatRuntimeConfig::default(),
            tempdir.path().to_path_buf(),
            tempdir.path().join("knowledge.db"),
            config.security.prompt_injection.clone(),
            hive_contracts::config::CommandPolicyConfig::default(),
            tempdir.path().join("risk-ledger.db"),
            model_router,
            CanvasSessionRegistry::new(),
            Default::default(),
            address.to_string(),
            Default::default(),
            None, // mcp
            None, // mcp_catalog
            connector_registry.clone(),
            None, // connector_audit_log
            connector_service,
            Arc::new(
                hive_scheduler::SchedulerService::in_memory(
                    event_bus.clone(),
                    hive_scheduler::SchedulerConfig::default(),
                )
                .expect("test scheduler"),
            ),
            Arc::new(parking_lot::RwLock::new(std::collections::HashMap::new())),
            Arc::new(parking_lot::RwLock::new(hive_contracts::SandboxConfig::default())),
            Arc::new(hive_contracts::DetectedShells::default()),
            hive_contracts::ToolLimitsConfig::default(),
            hive_contracts::CodeActConfig::default(),
            None, // plugin_host
            None, // plugin_registry
        ));
        let state = AppState::with_chat(config, audit, event_bus.clone(), shutdown.clone(), chat);
        state.start_background().await;
        let router = build_router(state);
        let server_shutdown = shutdown.clone();

        let handle = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    server_shutdown.notified().await;
                })
                .await
                .map_err(anyhow::Error::from)
        });

        Ok(TestDaemon {
            base_url: format!("http://{address}"),
            event_bus,
            shutdown,
            handle,
            tempdir,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_provider_returns_scripted_response() {
        let provider =
            MockProvider::new().on_contains("classify", "CONFIDENTIAL").default_response("DEFAULT");

        let response = provider.invoke("please classify this").await.expect("response");
        assert_eq!(response.content, "CONFIDENTIAL");
        assert_eq!(provider.call_count(), 1);
    }

    #[tokio::test]
    async fn test_daemon_serves_healthcheck() {
        let daemon = TestDaemon::spawn().await.expect("test daemon");

        let response =
            reqwest::get(format!("{}/healthz", daemon.base_url)).await.expect("health response");
        assert!(response.status().is_success());

        daemon.stop().await.expect("stop daemon");
    }
}
