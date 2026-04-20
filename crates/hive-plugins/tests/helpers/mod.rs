//! E2E test harness for the Hivemind plugin system.
//!
//! Provides `PluginTestEnv` — spawns the real test-plugin as a Node.js child
//! process and wires up in-memory secret/store/notification backends so tests
//! can exercise the full JSON-RPC round-trip without any external services.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde_json::{json, Value};

use hive_plugins::host::{HostHandler, PluginHost, PluginProcess};
use hive_plugins::manifest::PluginManifest;
use hive_plugins::protocol;

// ─── Test Environment ───────────────────────────────────────────────────────

/// Self-contained environment for E2E plugin tests.
///
/// Spawns the `@hivemind-os/test-plugin` as a real Node.js child process,
/// intercepts every host-API call, and exposes collected messages / events /
/// statuses so assertions can inspect them.
pub struct PluginTestEnv {
    pub host: PluginHost,
    pub plugin_id: String,
    pub process: Arc<PluginProcess>,
    pub messages: Arc<parking_lot::Mutex<Vec<Value>>>,
    pub events: Arc<parking_lot::Mutex<Vec<(String, Value)>>>,
    pub statuses: Arc<parking_lot::Mutex<Vec<Value>>>,
    pub secrets: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    pub store: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    pub notifications: Arc<parking_lot::Mutex<Vec<Value>>>,
    pub temp_dir: tempfile::TempDir,
}

impl PluginTestEnv {
    /// Create a test env with a sensible default config.
    pub async fn new() -> Result<Self> {
        let config = json!({
            "apiKey": "test-key",
            "endpoint": "https://httpbin.org",
            "pollInterval": 1,
            "failOnActivate": false
        });
        Self::with_config(config).await
    }

    /// Create a test env with a caller-supplied plugin config.
    pub async fn with_config(config: Value) -> Result<Self> {
        // ── temp directory for plugin data ──────────────────────────────
        let temp_dir = tempfile::tempdir().context("create temp dir")?;
        let data_dir = temp_dir.path().to_path_buf();

        // ── locate the test-plugin package ──────────────────────────────
        let plugin_dir = resolve_test_plugin_dir()?;
        let manifest = PluginManifest::from_package_json(&plugin_dir.join("package.json"))
            .context("parse test-plugin manifest")?;
        let plugin_id = manifest.plugin_id();
        let entry_point = manifest.main.clone();

        // ── shared state for the host handler ───────────────────────────
        let messages: Arc<parking_lot::Mutex<Vec<Value>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let events: Arc<parking_lot::Mutex<Vec<(String, Value)>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let statuses: Arc<parking_lot::Mutex<Vec<Value>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));
        let secrets: Arc<parking_lot::Mutex<HashMap<String, String>>> =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let store: Arc<parking_lot::Mutex<HashMap<String, String>>> =
            Arc::new(parking_lot::Mutex::new(HashMap::new()));
        let notifications: Arc<parking_lot::Mutex<Vec<Value>>> =
            Arc::new(parking_lot::Mutex::new(Vec::new()));

        // ── build the host handler closure ──────────────────────────────
        let handler = build_host_handler(
            messages.clone(),
            events.clone(),
            statuses.clone(),
            secrets.clone(),
            store.clone(),
            notifications.clone(),
        );

        // ── create host & spawn plugin ──────────────────────────────────
        let host = PluginHost::new(plugin_dir.clone(), data_dir).with_host_handler(handler);

        let process = host
            .spawn(
                &plugin_id,
                &plugin_dir,
                &entry_point,
                config.clone(),
                Some(&manifest.hivemind),
            )
            .await
            .context("spawn test plugin")?;

        // Activate
        host.activate(&plugin_id, Some(config))
            .await
            .context("activate test plugin")?;

        Ok(Self {
            host,
            plugin_id,
            process,
            messages,
            events,
            statuses,
            secrets,
            store,
            notifications,
            temp_dir,
        })
    }

    // ── convenience wrappers ────────────────────────────────────────────

    /// Call a tool exposed by the running plugin.
    pub async fn call_tool(&self, name: &str, args: Value) -> Result<Value> {
        self.host.call_tool(&self.plugin_id, name, args).await
    }

    /// Start the plugin's background polling loop.
    pub async fn start_loop(&self) -> Result<()> {
        self.host.start_loop(&self.plugin_id).await
    }

    /// Stop the plugin's background polling loop.
    pub async fn stop_loop(&self) -> Result<()> {
        self.host.stop_loop(&self.plugin_id).await
    }

    /// Poll until at least `count` messages have been captured,
    /// or `timeout` has elapsed.
    pub async fn wait_for_messages(&self, count: usize, timeout: std::time::Duration) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let msgs = self.messages.lock();
                if msgs.len() >= count {
                    return msgs.clone();
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return self.messages.lock().clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Poll until at least `count` events have been captured,
    /// or `timeout` has elapsed.
    pub async fn wait_for_events(
        &self,
        count: usize,
        timeout: std::time::Duration,
    ) -> Vec<(String, Value)> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            {
                let evts = self.events.lock();
                if evts.len() >= count {
                    return evts.clone();
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return self.events.lock().clone();
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Return the most-recently captured status update, if any.
    pub fn last_status(&self) -> Option<Value> {
        self.statuses.lock().last().cloned()
    }

    /// Gracefully stop the plugin and clean up.
    pub async fn shutdown(self) -> Result<()> {
        let _ = self.host.stop(&self.plugin_id).await;
        // temp_dir is cleaned up on drop
        Ok(())
    }
}

// ─── Helpers ────────────────────────────────────────────────────────────────

/// Resolve the path to the test-plugin package directory.
fn resolve_test_plugin_dir() -> Result<PathBuf> {
    // 1. Honour explicit override
    if let Ok(p) = std::env::var("HIVEMIND_TEST_PLUGIN_PATH") {
        let dir = PathBuf::from(p);
        anyhow::ensure!(dir.exists(), "HIVEMIND_TEST_PLUGIN_PATH does not exist: {}", dir.display());
        return Ok(dir);
    }

    // 2. Fall back to workspace-relative path
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dir = crate_dir.join("..").join("..").join("packages").join("test-plugin");
    let dir = dir
        .canonicalize()
        .with_context(|| format!("test-plugin not found at {}", dir.display()))?;

    // On Windows, canonicalize() produces \\?\ UNC paths that break Node.js.
    // Strip the prefix to get a normal path.
    #[cfg(windows)]
    let dir = {
        let s = dir.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            dir
        }
    };

    anyhow::ensure!(
        dir.join("package.json").exists(),
        "test-plugin package.json not found in {}",
        dir.display()
    );

    Ok(dir)
}

/// Build the `HostHandler` closure that implements every host API the
/// test-plugin might call, backed by the provided in-memory stores.
fn build_host_handler(
    messages: Arc<parking_lot::Mutex<Vec<Value>>>,
    events: Arc<parking_lot::Mutex<Vec<(String, Value)>>>,
    statuses: Arc<parking_lot::Mutex<Vec<Value>>>,
    secrets: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    store: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    notifications: Arc<parking_lot::Mutex<Vec<Value>>>,
) -> HostHandler {
    Arc::new(move |method: &str, params: Value| {
        // Clone Arcs into the future
        let messages = messages.clone();
        let events = events.clone();
        let statuses = statuses.clone();
        let secrets = secrets.clone();
        let store = store.clone();
        let notifications = notifications.clone();
        let method = method.to_string();

        Box::pin(async move {
            match method.as_str() {
                // ── forwarded from the reader task (for capture) ─────
                protocol::host_methods::EMIT_MESSAGE => {
                    // SDK sends { message: { channel, content, ... } }
                    let msg = params.get("message").cloned().unwrap_or(params.clone());
                    messages.lock().push(msg);
                    Ok(Value::Null)
                }
                protocol::host_methods::EMIT_MESSAGES => {
                    if let Some(arr) = params.get("messages").and_then(|v| v.as_array()) {
                        let mut msgs = messages.lock();
                        for m in arr {
                            msgs.push(m.clone());
                        }
                    }
                    Ok(Value::Null)
                }
                protocol::host_methods::EMIT_EVENT => {
                    let event_type = params["eventType"]
                        .as_str()
                        .unwrap_or("")
                        .to_string();
                    let payload = params
                        .get("payload")
                        .cloned()
                        .unwrap_or(Value::Null);
                    events.lock().push((event_type, payload));
                    Ok(Value::Null)
                }
                protocol::host_methods::UPDATE_STATUS => {
                    statuses.lock().push(params);
                    Ok(Value::Null)
                }

                // ── secrets ─────────────────────────────────────────
                protocol::host_methods::SECRET_GET => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let val = secrets.lock().get(&key).cloned();
                    Ok(json!({ "value": val }))
                }
                protocol::host_methods::SECRET_SET => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let value = params["value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    secrets.lock().insert(key, value);
                    Ok(Value::Null)
                }
                protocol::host_methods::SECRET_DELETE => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    secrets.lock().remove(&key);
                    Ok(Value::Null)
                }
                protocol::host_methods::SECRET_HAS => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let has = secrets.lock().contains_key(&key);
                    Ok(json!({ "exists": has }))
                }

                // ── key-value store ─────────────────────────────────
                protocol::host_methods::STORE_GET => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let val = store.lock().get(&key).cloned();
                    Ok(json!({ "value": val }))
                }
                protocol::host_methods::STORE_SET => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    let value = params["value"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    store.lock().insert(key, value);
                    Ok(Value::Null)
                }
                protocol::host_methods::STORE_DELETE => {
                    let key = params["key"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string();
                    store.lock().remove(&key);
                    Ok(Value::Null)
                }
                protocol::host_methods::STORE_KEYS => {
                    let keys: Vec<String> =
                        store.lock().keys().cloned().collect();
                    Ok(json!({ "keys": keys }))
                }

                // ── notifications ───────────────────────────────────
                protocol::host_methods::NOTIFY => {
                    notifications.lock().push(params);
                    Ok(Value::Null)
                }

                // ── HTTP fetch (best-effort real request) ───────────
                protocol::host_methods::HTTP_FETCH => {
                    handle_http_fetch(params).await
                }

                // ── logging (no-op, just trace) ─────────────────────
                protocol::host_methods::LOG => {
                    let level = params["level"].as_str().unwrap_or("info");
                    let log_msg = params["msg"].as_str().unwrap_or("");
                    tracing::info!(level, "[test-plugin] {}", log_msg);
                    Ok(Value::Null)
                }

                // ── catch-all ───────────────────────────────────────
                other => {
                    tracing::warn!("Unhandled host method in test harness: {}", other);
                    Err(anyhow::anyhow!("Unhandled host method: {}", other))
                }
            }
        })
    })
}

/// Best-effort HTTP fetch using reqwest.  Falls back to a mock on error.
async fn handle_http_fetch(params: Value) -> Result<Value> {
    let url = params["url"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let method = params["method"]
        .as_str()
        .unwrap_or("GET")
        .to_uppercase();
    let headers = params
        .get("headers")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let body = params.get("body").cloned();

    let client = reqwest::Client::new();
    let mut builder = match method.as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "PATCH" => client.patch(&url),
        "HEAD" => client.head(&url),
        _ => client.get(&url),
    };

    // Apply headers
    if let Some(obj) = headers.as_object() {
        for (k, v) in obj {
            if let Some(val) = v.as_str() {
                builder = builder.header(k.as_str(), val);
            }
        }
    }

    // Apply body
    if let Some(b) = body {
        builder = builder.json(&b);
    }

    match builder.send().await {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let resp_headers: HashMap<String, String> = resp
                .headers()
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        v.to_str().unwrap_or("").to_string(),
                    )
                })
                .collect();
            let body_text = resp.text().await.unwrap_or_default();
            Ok(json!({
                "status": status,
                "headers": resp_headers,
                "body": body_text,
            }))
        }
        Err(e) => {
            // Return a synthetic error response rather than failing
            Ok(json!({
                "status": 0,
                "headers": {},
                "body": format!("fetch error: {e}"),
            }))
        }
    }
}
