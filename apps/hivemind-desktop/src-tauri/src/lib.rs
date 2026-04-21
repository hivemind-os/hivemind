mod service_registration;
mod tray;
mod update;

use hive_contracts::{
    ChatMemoryItem, ChatSessionSnapshot, ChatSessionSummary, DiscoveredSkill, FileAuditRecord,
    FileAuditStatus, HardwareSummary, HubRepoFilesResult, HubSearchResult, InferenceParams,
    InferenceRuntimeKind, InstallModelRequest, InstalledModel, InstalledSkill, InterruptMode,
    InterruptRequest, LocalModelSummary, McpNotificationEvent, McpPromptInfo, McpResourceInfo,
    McpServerLog, McpServerSnapshot, McpToolInfo, MessageAttachment, ModelRouterSnapshot, Persona,
    RiskScanRecord, ScanDecision, SendMessageRequest, SendMessageResponse, SessionModality,
    SessionPermissions, SkillAuditResult, SkillSourceConfig, ToolApprovalRequest,
    ToolApprovalResponse, ToolDefinition, UserInteractionResponse, WorkspaceEntry,
    WorkspaceFileContent,
};
use hive_core::{
    config_to_yaml, daemon_start as start_daemon_process, daemon_status as fetch_daemon_status,
    daemon_stop as stop_daemon_process, daemon_url, discover_paths, load_config,
};
use reqwest::blocking::{Client, Response};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::io::{BufReader, BufWriter};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

struct AppState {
    paste_cancelled: Arc<std::sync::atomic::AtomicBool>,
    /// Set to `true` when an update is being installed so the close-to-tray
    /// handler allows the window to actually close (the NSIS installer needs
    /// the process to exit).
    update_installing: Arc<std::sync::atomic::AtomicBool>,
    /// Channel for the backend to send conflict questions and receive answers.
    paste_conflict_tx: Mutex<Option<std::sync::mpsc::Sender<PasteConflict>>>,
    paste_conflict_rx: Mutex<Option<std::sync::mpsc::Receiver<PasteConflict>>>,
    paste_conflict_response_tx: Mutex<Option<std::sync::mpsc::Sender<String>>>,
    paste_conflict_response_rx: Mutex<Option<std::sync::mpsc::Receiver<String>>>,
}

struct PasteConflict {
    _file_name: String,
    _destination: String,
}

#[derive(Clone, Copy, PartialEq)]
enum ConflictPolicy {
    Ask,
    ReplaceAll,
    SkipAll,
}
use tauri::{Emitter, Manager};

#[tauri::command(rename_all = "snake_case")]
fn write_frontend_log(level: String, source: String, message: String) -> Result<(), String> {
    let paths = discover_paths().map_err(|e| e.to_string())?;
    let log_path = paths.hivemind_home.join("hivemind-desktop.log");
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| e.to_string())?;
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    writeln!(file, "[{secs}.{millis:03}] [{level}] [{source}] {message}")
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Maximum size in bytes for SSE stream buffers to prevent unbounded growth.
const MAX_SSE_BUFFER_SIZE: usize = 1024 * 1024; // 1 MB

fn trim_sse_buffer(buffer: &mut String) {
    if buffer.len() <= MAX_SSE_BUFFER_SIZE {
        return;
    }
    if let Some(pos) = buffer.rfind("\n\n") {
        *buffer = buffer[pos + 2..].to_string();
    } else {
        buffer.clear();
    }
}

/// Drives an authenticated SSE event stream with optional reconnection.
///
/// Connects to `url` via GET using the shared daemon-authenticated HTTP
/// client, reads the `bytes_stream()`, and parses `\n\n`-delimited SSE
/// frames.  For each `data:` line the `on_event` callback receives the
/// raw payload string (prefix already stripped).  Return `false` from
/// `on_event` to terminate the stream immediately.
///
/// When `reconnect` is `true` the function re-connects with exponential
/// back-off (1 s → 30 s cap) after connection errors, non-success HTTP
/// responses, or stream termination.  HTTP 401 responses trigger a
/// daemon-token invalidation and up to three retries before giving up.
async fn sse_subscribe_loop<F>(url: String, reconnect: bool, on_event: F)
where
    F: Fn(&str) -> bool + Send,
{
    use tokio_stream::StreamExt;

    let client = shared_async_client().clone();
    let mut backoff = 1u64;
    let mut auth_retries = 0u32;

    loop {
        let response = match with_auth_async(client.get(&url)).send().await {
            Ok(r) => r,
            Err(_) => {
                if !reconnect {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            if reconnect && status == reqwest::StatusCode::UNAUTHORIZED {
                auth_retries += 1;
                if auth_retries <= 3 {
                    invalidate_daemon_token();
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(30);
                    continue;
                }
                return;
            }
            if !reconnect || (status.as_u16() >= 400 && status.as_u16() < 500) {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
            continue;
        }

        // Connected successfully — reset backoff.
        backoff = 1;
        auth_retries = 0;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut stop = false;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    trim_sse_buffer(&mut buffer);
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_block = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();
                        for line in event_block.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if !on_event(data) {
                                    stop = true;
                                    break;
                                }
                            }
                        }
                        if stop {
                            break;
                        }
                    }
                    if stop {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        if stop || !reconnect {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
        backoff = (backoff * 2).min(30);
    }
}

// Tracks active stream subscription tasks per session to prevent duplicates.
static ACTIVE_STREAMS: std::sync::LazyLock<
    Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Single global handle for the approval SSE stream.
static APPROVAL_STREAM: std::sync::LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Tracks active interaction SSE subscription (global, single stream).
static INTERACTION_STREAM: std::sync::LazyLock<
    Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
> = std::sync::LazyLock::new(|| Mutex::new(None));

/// Tracks active workflow event SSE subscription (global, single stream).
static WORKFLOW_EVENT_STREAM: std::sync::LazyLock<
    Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
> = std::sync::LazyLock::new(|| Mutex::new(None));

/// Tracks active MCP event SSE subscription (global, single stream).
static MCP_EVENT_STREAM: std::sync::LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Tracks active event-bus SSE subscription (global, single stream).
static EVENTBUS_STREAM: std::sync::LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Tracks active agent-stage SSE subscriptions per session.
static AGENT_STAGE_STREAMS: std::sync::LazyLock<
    Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Tracks active workspace index-status SSE subscriptions per session.
static INDEX_STATUS_STREAMS: std::sync::LazyLock<
    Mutex<HashMap<String, tauri::async_runtime::JoinHandle<()>>>,
> = std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, serde::Serialize)]
struct AppContextPayload {
    daemon_url: String,
    config_path: String,
    knowledge_graph_path: String,
    risk_ledger_path: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AuditStatusResponse {
    record: Option<FileAuditRecord>,
    status: FileAuditStatus,
}

#[tauri::command(rename_all = "snake_case")]
async fn daemon_status() -> Result<Option<hive_contracts::DaemonStatus>, String> {
    let url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || fetch_daemon_status(&url).ok())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn daemon_start() -> Result<hive_contracts::DaemonStatus, String> {
    let url = daemon_url(None).map_err(|error| error.to_string())?;
    let status_url = url.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let _ = start_daemon_process(&url, None)?;
        fetch_daemon_status(&status_url)
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn daemon_stop() -> Result<(), String> {
    let url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        stop_daemon_process(&url)?;
        // Wait for the daemon to fully exit so callers (e.g. the updater)
        // can safely replace the binary on disk.
        for _ in 0..25 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if fetch_daemon_status(&url).is_err() {
                return Ok(());
            }
        }
        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error: anyhow::Error| error.to_string())
}

/// Signal that an update install is about to begin so the close-to-tray
/// handler allows the window to actually close (the NSIS installer needs
/// the app process to exit).
#[tauri::command(rename_all = "snake_case")]
fn set_update_installing(state: tauri::State<'_, AppState>) {
    state.update_installing.store(true, std::sync::atomic::Ordering::SeqCst);
}

#[tauri::command(rename_all = "snake_case")]
fn open_url(url: String) -> Result<(), String> {
    open::that(&url).map_err(|e| format!("Failed to open URL: {e}"))
}

#[tauri::command(rename_all = "snake_case")]
fn config_show() -> Result<String, String> {
    load_config().and_then(|config| config_to_yaml(&config)).map_err(|error| error.to_string())
}

/// Return the daemon config as JSON (proxied through Tauri so auth is handled by the Rust layer).
#[tauri::command(rename_all = "snake_case")]
async fn config_get() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/config/get")
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Save config via daemon API (proxied through Tauri so auth is handled by the Rust layer).
#[tauri::command(rename_all = "snake_case")]
async fn config_save(
    app: tauri::AppHandle,
    config: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/config",
            config,
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "config");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
fn canvas_ws_url(session_id: String) -> Result<String, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    // Convert http(s):// to ws(s)://
    let ws_url = if base_url.starts_with("https://") {
        base_url.replacen("https://", "wss://", 1)
    } else {
        base_url.replacen("http://", "ws://", 1)
    };
    Ok(format!("{ws_url}/api/v1/canvas/{session_id}/ws"))
}

#[tauri::command(rename_all = "snake_case")]
fn app_context() -> Result<AppContextPayload, String> {
    let paths = discover_paths().map_err(|error| error.to_string())?;
    let daemon_url = daemon_url(None).map_err(|error| error.to_string())?;

    Ok(AppContextPayload {
        daemon_url,
        config_path: paths.config_path.display().to_string(),
        knowledge_graph_path: paths.knowledge_graph_path.display().to_string(),
        risk_ledger_path: paths.risk_ledger_path.display().to_string(),
    })
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_list_sessions() -> Result<Vec<ChatSessionSummary>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<ChatSessionSummary>>(&base_url, "/api/v1/chat/sessions")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_create_session(
    modality: Option<String>,
    persona_id: Option<String>,
) -> Result<ChatSessionSnapshot, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let modality_enum = match modality.as_deref() {
        Some("spatial") => SessionModality::Spatial,
        _ => SessionModality::Linear,
    };
    tauri::async_runtime::spawn_blocking(move || {
        let mut body = serde_json::json!({ "modality": modality_enum });
        if let Some(pid) = persona_id {
            body["persona_id"] = serde_json::Value::String(pid);
        }
        blocking_post_json::<serde_json::Value, ChatSessionSnapshot>(
            &base_url,
            "/api/v1/chat/sessions",
            body,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_set_session_persona(
    session_id: String,
    persona_id: String,
) -> Result<ChatSessionSnapshot, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, ChatSessionSnapshot>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/persona"),
            serde_json::json!({ "persona_id": persona_id }),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_get_session(session_id: String) -> Result<ChatSessionSnapshot, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<ChatSessionSnapshot>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_rename_session(
    session_id: String,
    title: String,
) -> Result<ChatSessionSnapshot, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_patch_json::<serde_json::Value, ChatSessionSnapshot>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}"),
            serde_json::json!({ "title": title }),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_session_agents(session_id: String) -> Result<Vec<serde_json::Value>, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/agents"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn pause_session_agent(session_id: String, agent_id: String) -> Result<(), String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/pause",
                urlencoding::encode(&agent_id)
            ),
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn resume_session_agent(session_id: String, agent_id: String) -> Result<(), String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/resume",
                urlencoding::encode(&agent_id)
            ),
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn kill_session_agent(session_id: String, agent_id: String) -> Result<(), String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/kill",
                urlencoding::encode(&agent_id)
            ),
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_agent_telemetry(session_id: String) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/agents/telemetry"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_agent_events(
    session_id: String,
    agent_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let off = offset.unwrap_or(0);
    let lim = limit.unwrap_or(50);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/events?offset={off}&limit={lim}",
                urlencoding::encode(&agent_id)
            ),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_session_events(
    session_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let off = offset.unwrap_or(0);
    let lim = limit.unwrap_or(500);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/events?offset={off}&limit={lim}"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn restart_session_agent(
    session_id: String,
    agent_id: String,
    model: Option<String>,
    allowed_tools: Option<Vec<String>>,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut body = serde_json::Map::new();
        if let Some(m) = model {
            body.insert("model".into(), serde_json::Value::String(m));
        }
        if let Some(tools) = allowed_tools {
            body.insert(
                "allowed_tools".into(),
                serde_json::Value::Array(
                    tools.into_iter().map(serde_json::Value::String).collect(),
                ),
            );
        }
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/restart",
                urlencoding::encode(&agent_id)
            ),
            serde_json::Value::Object(body),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

// ── Background process management ─────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn list_session_processes(session_id: String) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/processes"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn kill_process(process_id: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/processes/{process_id}/kill"),
            serde_json::json!({}),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_process_status(
    process_id: String,
    tail_lines: Option<usize>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let tl = tail_lines.unwrap_or(50);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/processes/{process_id}/status?tail_lines={tl}"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn process_subscribe_events(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!(
        "{}/api/v1/chat/sessions/{}/processes/events",
        base_url,
        urlencoding::encode(&session_id)
    );

    tokio::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("process:event", data.to_string());
            true
        })
        .await;
    });

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_stage_subscribe(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!(
        "{}/api/v1/chat/sessions/{}/agents/stream",
        base_url,
        urlencoding::encode(&session_id)
    );

    // Cancel any existing subscription for this session.
    if let Ok(mut streams) = AGENT_STAGE_STREAMS.lock() {
        if let Some(prev) = streams.remove(&session_id) {
            prev.abort();
        }
    }

    let sid = session_id.clone();
    let sid_cleanup = session_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let client = shared_async_client().clone();
        let mut backoff = 1u64;
        let mut auth_retries = 0u32;

        loop {
            let response = match with_auth_async(client.get(&url)).send().await {
                Ok(r) => r,
                Err(_) => {
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(30);
                    continue;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let body = match response.text().await {
                    Ok(body) => body,
                    Err(e) => format!("<failed to read body: {e}>"),
                };
                let _ = app.emit(
                    "stage:error",
                    serde_json::json!({ "session_id": sid, "error": format!("{status}: {body}") }),
                );
                // On 401, the daemon likely restarted with a new token.
                // Invalidate and retry up to 3 times.
                if status == reqwest::StatusCode::UNAUTHORIZED {
                    auth_retries += 1;
                    if auth_retries <= 3 {
                        invalidate_daemon_token();
                        tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                        backoff = (backoff * 2).min(30);
                        continue;
                    }
                    break;
                }
                // Don't retry on 4xx errors (session not found, etc.)
                if status.as_u16() >= 400 && status.as_u16() < 500 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
                continue;
            }

            backoff = 1;

            use tokio_stream::StreamExt;

            let mut stream = response.bytes_stream();
            let mut buffer = String::new();

            while let Some(chunk_result) = stream.next().await {
                match chunk_result {
                    Ok(bytes) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));
                        trim_sse_buffer(&mut buffer);
                        while let Some(pos) = buffer.find("\n\n") {
                            let event_block = buffer[..pos].to_string();
                            buffer = buffer[pos + 2..].to_string();
                            for line in event_block.lines() {
                                if let Some(data) = line.strip_prefix("data: ") {
                                    if let Ok(event) =
                                        serde_json::from_str::<serde_json::Value>(data)
                                    {
                                        let _ = app.emit(
                                            "stage:event",
                                            serde_json::json!({
                                                "session_id": sid,
                                                "event": event,
                                            }),
                                        );
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        let _ = app.emit(
                            "stage:error",
                            serde_json::json!({ "session_id": sid, "error": e.to_string() }),
                        );
                        break;
                    }
                }
            }

            // Stream ended — reconnect after a short delay
            tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }

        if let Ok(mut streams) = AGENT_STAGE_STREAMS.lock() {
            streams.remove(&sid_cleanup);
        }
    });

    if let Ok(mut streams) = AGENT_STAGE_STREAMS.lock() {
        streams.insert(session_id, handle);
    }

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_approve_tool(
    session_id: String,
    agent_id: String,
    request_id: String,
    approved: bool,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = UserInteractionResponse {
            request_id,
            payload: hive_contracts::InteractionResponsePayload::ToolApproval {
                approved,
                allow_session: false,
                allow_agent: false,
            },
        };
        blocking_post_json::<UserInteractionResponse, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/interaction",
                urlencoding::encode(&agent_id)
            ),
            body,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_respond_interaction(
    session_id: String,
    agent_id: String,
    response: UserInteractionResponse,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<UserInteractionResponse, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/agents/{}/interaction",
                urlencoding::encode(&agent_id)
            ),
            response,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_pending_approvals() -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/pending-approvals")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_session_pending_questions(
    session_id: String,
) -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/pending-questions"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_all_pending_questions() -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/pending-questions")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_pending_interactions() -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/pending-interactions")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_pending_interaction_counts() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/pending-interaction-counts")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_user_status() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/status")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn set_user_status(status: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/status",
            serde_json::json!({ "status": status }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn status_heartbeat() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(&base_url, "/api/v1/status/heartbeat")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn subscribe_approval_stream(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/approval-events");

    // Cancel any existing subscription.
    if let Ok(mut guard) = APPROVAL_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                let _ = app.emit("approval:event", event);
            }
            true
        })
        .await;
    });

    if let Ok(mut guard) = APPROVAL_STREAM.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn interactions_subscribe(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/interactions/stream");

    // Cancel any existing subscription.
    if let Ok(mut guard) = INTERACTION_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("interaction:event", data.to_string());
            true
        })
        .await;
    });

    if let Ok(mut guard) = INTERACTION_STREAM.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_delete_session(session_id: String, scrub_kb: Option<bool>) -> Result<(), String> {
    validate_id(&session_id)?;

    // Abort any SSE reconnect loops for this session before deleting
    if let Ok(mut streams) = AGENT_STAGE_STREAMS.lock() {
        if let Some(handle) = streams.remove(&session_id) {
            handle.abort();
        }
    }
    if let Ok(mut streams) = ACTIVE_STREAMS.lock() {
        if let Some(handle) = streams.remove(&session_id) {
            handle.abort();
        }
    }

    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let scrub = scrub_kb.unwrap_or(false);
    tauri::async_runtime::spawn_blocking(move || {
        let path = if scrub {
            format!("/api/v1/chat/sessions/{session_id}?scrub_kb=true")
        } else {
            format!("/api/v1/chat/sessions/{session_id}")
        };
        blocking_delete::<serde_json::Value>(&base_url, &path).map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
async fn chat_send_message(
    session_id: String,
    content: String,
    scan_decision: Option<ScanDecision>,
    preferred_model: Option<String>,
    data_class_override: Option<String>,
    agent_id: Option<String>,
    canvas_position: Option<(f64, f64)>,
    excluded_tools: Option<Vec<String>>,
    excluded_skills: Option<Vec<String>>,
    attachments: Option<Vec<MessageAttachment>>,
    skip_preempt: Option<bool>,
) -> Result<SendMessageResponse, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<SendMessageRequest, SendMessageResponse>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/messages"),
            SendMessageRequest {
                content,
                scan_decision,
                preferred_models: preferred_model.map(|m| vec![m]),
                data_class_override,
                agent_id,
                role: Default::default(),
                canvas_position,
                excluded_tools,
                excluded_skills,
                attachments: attachments.unwrap_or_default(),
                skip_preempt,
            },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_approve_tool(
    session_id: String,
    request_id: String,
    approved: bool,
    allow_session: Option<bool>,
    allow_agent: Option<bool>,
) -> Result<ToolApprovalResponse, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<ToolApprovalRequest, ToolApprovalResponse>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/tool-approval"),
            ToolApprovalRequest {
                request_id,
                approved,
                allow_session: allow_session.unwrap_or(false),
                allow_agent: allow_agent.unwrap_or(false),
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_respond_interaction(
    session_id: String,
    response: UserInteractionResponse,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<UserInteractionResponse, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/interaction"),
            response,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_interrupt(
    session_id: String,
    mode: InterruptMode,
) -> Result<ChatSessionSnapshot, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<InterruptRequest, ChatSessionSnapshot>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/interrupt"),
            InterruptRequest { mode },
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_session_permissions(session_id: String) -> Result<SessionPermissions, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<SessionPermissions>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/permissions"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn set_session_permissions(
    session_id: String,
    permissions: SessionPermissions,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json_no_content(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/permissions"),
            permissions,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_resume(session_id: String) -> Result<ChatSessionSnapshot, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<ChatSessionSnapshot>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/resume"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn recluster_canvas(session_id: String) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/recluster"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn propose_layout(
    session_id: String,
    algorithm: Option<String>,
) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = serde_json::json!({ "algorithm": algorithm });
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/propose-layout"),
            body,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_get_session_memory(session_id: String) -> Result<Vec<ChatMemoryItem>, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<ChatMemoryItem>>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/memory"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_upload_file(
    session_id: String,
    file_name: String,
    content: String,
) -> Result<String, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = serde_json::json!({
            "filename": file_name,
            "content": content,
        });
        let resp = blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/upload"),
            body,
        )?;
        Ok(resp.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_link_workspace(session_id: String, path: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = serde_json::json!({ "path": path });
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/link-workspace"),
            body,
        )
        .map(|_| ())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_list_files(
    session_id: String,
    path: Option<String>,
) -> Result<Vec<WorkspaceEntry>, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let url = match path {
            Some(ref p) => {
                let encoded = encode_query(p);
                format!("/api/v1/chat/sessions/{session_id}/workspace/files?path={encoded}")
            }
            None => format!("/api/v1/chat/sessions/{session_id}/workspace/files"),
        };
        blocking_get_json::<Vec<WorkspaceEntry>>(&base_url, &url)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_read_file(
    session_id: String,
    path: String,
) -> Result<WorkspaceFileContent, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<WorkspaceFileContent>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/file?path={encoded_path}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_save_file(
    session_id: String,
    path: String,
    content: String,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/file?path={encoded_path}"),
            serde_json::json!({ "content": content }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_create_directory(session_id: String, path: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/directory?path={encoded_path}"),
            serde_json::json!({}),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_delete_entry(session_id: String, path: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_delete::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/entry?path={encoded_path}"),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_move_entry(session_id: String, from: String, to: String) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/move"),
            serde_json::json!({ "from": from, "to": to }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
fn clipboard_copy_files(paths: Vec<String>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        clipboard_copy_files_macos(paths)
    }

    #[cfg(target_os = "windows")]
    {
        clipboard_copy_files_windows(paths)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = paths;
        Err("clipboard file copy is not supported on this platform".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
fn clipboard_read_file_paths() -> Result<Vec<String>, String> {
    #[cfg(target_os = "macos")]
    {
        clipboard_read_file_paths_macos()
    }

    #[cfg(target_os = "windows")]
    {
        clipboard_read_file_paths_windows()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("clipboard file paste is not supported on this platform".to_string())
    }
}

#[tauri::command(rename_all = "snake_case")]
async fn clipboard_paste_files(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    session_id: String,
    target_dir: String,
    source_paths: Vec<String>,
) -> Result<(), String> {
    validate_id(&session_id)?;

    state.paste_cancelled.store(false, std::sync::atomic::Ordering::Relaxed);
    let cancelled = state.paste_cancelled.clone();

    // Set up conflict channels.
    let (conflict_tx, conflict_rx) = std::sync::mpsc::channel::<PasteConflict>();
    let (response_tx, response_rx) = std::sync::mpsc::channel::<String>();
    *state.paste_conflict_tx.lock().map_err(|_| "paste conflict tx lock poisoned")? =
        Some(conflict_tx);
    *state.paste_conflict_rx.lock().map_err(|_| "paste conflict rx lock poisoned")? =
        Some(conflict_rx);
    *state
        .paste_conflict_response_tx
        .lock()
        .map_err(|_| "paste conflict response tx lock poisoned")? = Some(response_tx);
    *state
        .paste_conflict_response_rx
        .lock()
        .map_err(|_| "paste conflict response rx lock poisoned")? = Some(response_rx);

    let result = tauri::async_runtime::spawn_blocking(move || {
        let workspace_root = resolve_workspace_path(&session_id)?;
        let target_root = workspace_root.join(normalize_workspace_target_path(&target_dir)?);
        ensure_path_within_workspace(&target_root, &workspace_root)?;
        std::fs::create_dir_all(&target_root).map_err(|error| error.to_string())?;

        let total: usize = source_paths.iter().map(|p| count_files_recursive(Path::new(p))).sum();
        let copied = std::sync::atomic::AtomicUsize::new(0);
        let policy = Mutex::new(ConflictPolicy::Ask);

        for source_path in &source_paths {
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = app.emit(
                    "paste:progress",
                    serde_json::json!({ "session_id": session_id, "cancelled": true }),
                );
                return Ok(());
            }

            let source = PathBuf::from(source_path);
            if !source.is_absolute() {
                return Err(format!("clipboard source path must be absolute: {source_path}"));
            }
            let file_name = source.file_name().ok_or_else(|| {
                format!("clipboard source path has no final path component: {source_path}")
            })?;

            let destination = target_root.join(file_name);
            copy_path_into_workspace_progress(
                &source,
                &destination,
                &workspace_root,
                &app,
                &session_id,
                total,
                &copied,
                &cancelled,
                &policy,
            )?;
        }

        let _ = app.emit(
            "paste:progress",
            serde_json::json!({
                "session_id": session_id,
                "current": total,
                "total": total,
                "done": true,
            }),
        );

        Ok(())
    })
    .await
    .map_err(|error| error.to_string())?;

    // Clean up channels.
    *state.paste_conflict_tx.lock().map_err(|_| "paste conflict tx lock poisoned")? = None;
    *state.paste_conflict_rx.lock().map_err(|_| "paste conflict rx lock poisoned")? = None;
    *state
        .paste_conflict_response_tx
        .lock()
        .map_err(|_| "paste conflict response tx lock poisoned")? = None;
    *state
        .paste_conflict_response_rx
        .lock()
        .map_err(|_| "paste conflict response rx lock poisoned")? = None;

    result
}

#[tauri::command(rename_all = "snake_case")]
fn clipboard_cancel_paste(state: tauri::State<'_, AppState>) {
    state.paste_cancelled.store(true, std::sync::atomic::Ordering::Relaxed);
    // Also unblock any waiting conflict prompt.
    if let Ok(guard) = state.paste_conflict_response_tx.lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send("cancel".to_string());
        }
    }
}

#[tauri::command(rename_all = "snake_case")]
fn clipboard_resolve_conflict(state: tauri::State<'_, AppState>, resolution: String) {
    if let Ok(guard) = state.paste_conflict_response_tx.lock() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(resolution);
        }
    }
}

fn count_files_recursive(path: &Path) -> usize {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return 1;
    }
    if meta.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            return 0;
        };
        return entries.filter_map(|e| e.ok()).map(|e| count_files_recursive(&e.path())).sum();
    }
    0
}

/// Ask the frontend what to do about a conflict.
/// Returns true if the file should be replaced, false if skipped.
fn resolve_conflict(
    app: &tauri::AppHandle,
    session_id: &str,
    file_name: &str,
    destination: &str,
    policy: &Mutex<ConflictPolicy>,
) -> Result<bool, String> {
    let current_policy = *policy.lock().map_err(|_| "conflict policy lock poisoned")?;
    match current_policy {
        ConflictPolicy::ReplaceAll => return Ok(true),
        ConflictPolicy::SkipAll => return Ok(false),
        ConflictPolicy::Ask => {}
    }

    // Emit conflict event to frontend.
    let _ = app.emit(
        "paste:conflict",
        serde_json::json!({
            "session_id": session_id,
            "file_name": file_name,
            "destination": destination,
        }),
    );

    // Wait for frontend response (blocks the paste thread).
    // The frontend will call clipboard_resolve_conflict with one of:
    // "replace", "skip", "replace_all", "skip_all", "cancel"
    let rx_guard = app.state::<AppState>();
    let rx_lock = rx_guard
        .paste_conflict_response_rx
        .lock()
        .map_err(|_| "conflict response lock poisoned")?;
    let rx = rx_lock.as_ref().ok_or("no conflict channel")?;
    let response = rx.recv().map_err(|_| "conflict channel closed".to_string())?;

    match response.as_str() {
        "replace" => Ok(true),
        "skip" => Ok(false),
        "replace_all" => {
            *policy.lock().map_err(|_| "conflict policy lock poisoned")? =
                ConflictPolicy::ReplaceAll;
            Ok(true)
        }
        "skip_all" => {
            *policy.lock().map_err(|_| "conflict policy lock poisoned")? = ConflictPolicy::SkipAll;
            Ok(false)
        }
        _ => {
            // "cancel" or unknown
            Err("paste cancelled".to_string())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn copy_path_into_workspace_progress(
    source: &Path,
    destination: &Path,
    workspace_root: &Path,
    app: &tauri::AppHandle,
    session_id: &str,
    total: usize,
    copied: &std::sync::atomic::AtomicUsize,
    cancelled: &std::sync::atomic::AtomicBool,
    policy: &Mutex<ConflictPolicy>,
) -> Result<(), String> {
    if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }

    let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!("clipboard paste does not support symlinks: {}", source.display()));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| format!("invalid destination path: {}", destination.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    ensure_path_within_workspace(parent, workspace_root)?;

    if metadata.is_dir() {
        // For directories, don't blindly remove — recurse into children so
        // individual file conflicts are caught.
        std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        ensure_path_within_workspace(destination, workspace_root)?;

        for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_path_into_workspace_progress(
                &entry.path(),
                &destination.join(entry.file_name()),
                workspace_root,
                app,
                session_id,
                total,
                copied,
                cancelled,
                policy,
            )?;
        }
        return Ok(());
    }

    if !metadata.is_file() {
        return Err(format!(
            "clipboard paste only supports files and directories: {}",
            source.display()
        ));
    }

    // Check for conflict before copying.
    if destination.exists() {
        let file_name =
            source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let dest_relative = destination
            .strip_prefix(workspace_root)
            .unwrap_or(destination)
            .to_string_lossy()
            .to_string();
        let should_replace = resolve_conflict(app, session_id, &file_name, &dest_relative, policy)?;
        if !should_replace {
            // Skip this file but still count it for progress.
            let current = copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let _ = app.emit(
                "paste:progress",
                serde_json::json!({
                    "session_id": session_id,
                    "current": current,
                    "total": total,
                    "file_name": file_name,
                    "skipped": true,
                }),
            );
            return Ok(());
        }
        remove_existing_destination(destination, workspace_root)?;
    }

    copy_file_buffered(source, destination)?;

    let current = copied.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let file_name = source.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let _ = app.emit(
        "paste:progress",
        serde_json::json!({
            "session_id": session_id,
            "current": current,
            "total": total,
            "file_name": file_name,
        }),
    );

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_audit_file(
    session_id: String,
    path: String,
    model: String,
) -> Result<FileAuditRecord, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, FileAuditRecord>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/audit?path={encoded_path}"),
            serde_json::json!({ "model": model }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_get_audit(
    session_id: String,
    path: String,
) -> Result<AuditStatusResponse, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<AuditStatusResponse>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/audit?path={encoded_path}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_get_classification(session_id: String) -> Result<serde_json::Value, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/classification"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_set_classification_default(
    session_id: String,
    default: String,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/workspace/classification"),
            serde_json::json!({ "default": default }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_set_classification_override(
    session_id: String,
    path: String,
    class: String,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/workspace/classification/override?path={encoded_path}"
            ),
            serde_json::json!({ "class": class }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_clear_classification_override(
    session_id: String,
    path: String,
) -> Result<(), String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let encoded_path = encode_query(&path);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_delete::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/chat/sessions/{session_id}/workspace/classification/override?path={encoded_path}"
            ),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn memory_search(query: String) -> Result<Vec<ChatMemoryItem>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<ChatMemoryItem>>(
            &base_url,
            &format!("/api/v1/memory/search?query={}", encode_query(&query)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_list_risk_scans(session_id: String) -> Result<Vec<RiskScanRecord>, String> {
    validate_id(&session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<RiskScanRecord>>(
            &base_url,
            &format!("/api/v1/chat/sessions/{session_id}/risk-scans"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn model_router_snapshot() -> Result<ModelRouterSnapshot, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<ModelRouterSnapshot>(&base_url, "/api/v1/model/router")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_list_servers() -> Result<Vec<McpServerSnapshot>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpServerSnapshot>>(&base_url, "/api/v1/mcp/servers")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_connect_server(server_id: String) -> Result<McpServerSnapshot, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<McpServerSnapshot>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/connect"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_disconnect_server(server_id: String) -> Result<McpServerSnapshot, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<McpServerSnapshot>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/disconnect"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_list_tools(server_id: String) -> Result<Vec<McpToolInfo>, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpToolInfo>>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/tools"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_list_resources(server_id: String) -> Result<Vec<McpResourceInfo>, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpResourceInfo>>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/resources"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_list_prompts(server_id: String) -> Result<Vec<McpPromptInfo>, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpPromptInfo>>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/prompts"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_list_notifications(limit: Option<usize>) -> Result<Vec<McpNotificationEvent>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let limit = limit.unwrap_or(25).min(1000);
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpNotificationEvent>>(
            &base_url,
            &format!("/api/v1/mcp/notifications?limit={limit}"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_server_logs(server_id: String) -> Result<Vec<McpServerLog>, String> {
    validate_id(&server_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<McpServerLog>>(
            &base_url,
            &format!("/api/v1/mcp/servers/{server_id}/logs"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Proxy MCP registry searches through the Rust backend so the Tauri
/// webview doesn't need to fetch external HTTPS URLs directly (which is
/// blocked as mixed-content in production builds).
#[tauri::command(rename_all = "snake_case")]
async fn mcp_registry_search(
    search: Option<String>,
    cursor: Option<String>,
    limit: Option<u32>,
) -> Result<serde_json::Value, String> {
    let mut url = reqwest::Url::parse("https://registry.modelcontextprotocol.io/v0.1/servers")
        .map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("version", "latest");
    if let Some(ref q) = search {
        if !q.is_empty() {
            url.query_pairs_mut().append_pair("search", q);
        }
    }
    if let Some(ref c) = cursor {
        url.query_pairs_mut().append_pair("cursor", c);
    }
    url.query_pairs_mut().append_pair("limit", &limit.unwrap_or(30).to_string());

    let client = shared_async_client();
    let resp =
        client.get(url).timeout(std::time::Duration::from_secs(30)).send().await.map_err(|e| {
            if e.is_timeout() {
                "Registry request timed out. Check your network connection.".to_string()
            } else {
                format!("Registry request failed: {e}")
            }
        })?;
    if !resp.status().is_success() {
        return Err(format!("Registry search failed: {}", resp.status()));
    }
    resp.json::<serde_json::Value>().await.map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn tools_list(persona_id: Option<String>) -> Result<Vec<ToolDefinition>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = match persona_id {
            Some(ref pid) => format!("/api/v1/tools?persona_id={}", urlencoding::encode(pid)),
            None => "/api/v1/tools".to_string(),
        };
        blocking_get_json::<Vec<ToolDefinition>>(&base_url, &path)
    })
    .await
    .map_err(|error| error.to_string())?
}

// ---- Local model management ------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
async fn local_models_list() -> Result<LocalModelSummary, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<LocalModelSummary>(&base_url, "/api/v1/local-models")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_get(model_id: String) -> Result<InstalledModel, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<InstalledModel>(
            &base_url,
            &format!("/api/v1/local-models/{}", encode_query(&model_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_install(
    app: tauri::AppHandle,
    hub_repo: String,
    filename: String,
    runtime: InferenceRuntimeKind,
) -> Result<InstalledModel, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<InstallModelRequest, InstalledModel>(
            &base_url,
            "/api/v1/local-models/install",
            InstallModelRequest { hub_repo, filename, runtime, capabilities: None },
        )
    })
    .await
    .map_err(|error| error.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "models");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_update_params(
    app: tauri::AppHandle,
    model_id: String,
    params: InferenceParams,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<InferenceParams, serde_json::Value>(
            &base_url,
            &format!("/api/v1/local-models/{}/params", encode_query(&model_id)),
            params,
        )
    })
    .await
    .map_err(|error| error.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "models");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_remove(
    app: tauri::AppHandle,
    model_id: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_delete::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/local-models/{}", encode_query(&model_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "models");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_search(
    query: Option<String>,
    task: Option<String>,
    limit: Option<usize>,
) -> Result<HubSearchResult, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut params = Vec::new();
        if let Some(q) = query {
            params.push(format!("query={}", encode_query(&q)));
        }
        if let Some(t) = task {
            params.push(format!("task={}", encode_query(&t)));
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        let qs = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
        blocking_get_json::<HubSearchResult>(&base_url, &format!("/api/v1/local-models/search{qs}"))
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_hub_files(repo_id: String) -> Result<HubRepoFilesResult, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<HubRepoFilesResult>(
            &base_url,
            &format!("/api/v1/local-models/hub/{}/files", encode_query(&repo_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_hardware() -> Result<HardwareSummary, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<HardwareSummary>(&base_url, "/api/v1/local-models/hardware")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DownloadProgress {
    model_id: String,
    repo_id: String,
    filename: String,
    total_bytes: Option<u64>,
    downloaded_bytes: u64,
    status: String,
    error: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_downloads() -> Result<Vec<DownloadProgress>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<DownloadProgress>>(&base_url, "/api/v1/local-models/downloads")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_remove_download(model_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_delete_no_content(
            &base_url,
            &format!("/api/v1/local-models/downloads/{}", encode_query(&model_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PerModelUsage {
    model_id: String,
    memory_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct DesktopResourceUsage {
    loaded_models: u32,
    total_memory_used_bytes: u64,
    per_model: Vec<PerModelUsage>,
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_resource_usage() -> Result<DesktopResourceUsage, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let hw = blocking_get_json::<HardwareSummary>(&base_url, "/api/v1/local-models/hardware")?;
        Ok(DesktopResourceUsage {
            loaded_models: hw.usage.models_loaded,
            total_memory_used_bytes: hw.usage.ram_used_bytes + hw.usage.vram_used_bytes,
            per_model: Vec::new(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_models_storage() -> Result<u64, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let summary = blocking_get_json::<LocalModelSummary>(&base_url, "/api/v1/local-models")?;
        Ok(summary.total_size_bytes)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Tracks the single global bots SSE stream.
static BOT_STREAM: std::sync::LazyLock<Mutex<Option<tauri::async_runtime::JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

#[tauri::command(rename_all = "snake_case")]
async fn list_bots() -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/bots")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn launch_bot(config: serde_json::Value) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/bots",
            config,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn message_bot(agent_id: String, content: String) -> Result<(), String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json_no_content(
            &base_url,
            &format!("/api/v1/bots/{}/message", urlencoding::encode(&agent_id)),
            serde_json::json!({ "content": content }),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn render_prompt_template(
    persona_id: String,
    prompt_id: String,
    params: serde_json::Value,
) -> Result<serde_json::Value, String> {
    validate_persona_id(&persona_id)?;
    validate_id(&prompt_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<_, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/config/personas/{}/prompts/{}/render",
                urlencoding::encode(&persona_id),
                urlencoding::encode(&prompt_id)
            ),
            serde_json::json!({ "params": params }),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn send_prompt_to_bot(
    agent_id: String,
    persona_id: String,
    prompt_id: String,
    params: serde_json::Value,
) -> Result<(), String> {
    validate_id(&agent_id)?;
    validate_persona_id(&persona_id)?;
    validate_id(&prompt_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json_no_content(
            &base_url,
            &format!("/api/v1/bots/{}/send-prompt", urlencoding::encode(&agent_id)),
            serde_json::json!({
                "persona_id": persona_id,
                "prompt_id": prompt_id,
                "params": params,
            }),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn deactivate_bot(agent_id: String) -> Result<(), String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_no_content(
            &base_url,
            &format!("/api/v1/bots/{}/deactivate", urlencoding::encode(&agent_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn activate_bot(agent_id: String) -> Result<(), String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_no_content(
            &base_url,
            &format!("/api/v1/bots/{}/activate", urlencoding::encode(&agent_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn delete_bot(agent_id: String) -> Result<(), String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_delete_no_content(
            &base_url,
            &format!("/api/v1/bots/{}", urlencoding::encode(&agent_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Ensure bot SSE stream is connected. Unlike bot_subscribe, this is
/// idempotent — it only starts a new connection if one isn't already running.
#[tauri::command(rename_all = "snake_case")]
async fn ensure_bot_stream(app: tauri::AppHandle) -> Result<(), String> {
    let already_running = BOT_STREAM.lock().map(|guard| guard.is_some()).unwrap_or(false);
    if already_running {
        return Ok(());
    }
    bot_subscribe(app).await
}

#[tauri::command(rename_all = "snake_case")]
async fn bot_subscribe(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/bots/stream");

    // Cancel any existing subscription.
    if let Ok(mut guard) = BOT_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                let _ = app.emit(
                    "stage:event",
                    serde_json::json!({
                        "session_id": "__service__",
                        "event": event,
                    }),
                );
            }
            true
        })
        .await;
    });

    if let Ok(mut guard) = BOT_STREAM.lock() {
        *guard = Some(handle);
    }

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn get_bot_events(
    agent_id: String,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<serde_json::Value, String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let off = offset.unwrap_or(0);
    let lim = limit.unwrap_or(50);
    let encoded_id = urlencoding::encode(&agent_id).into_owned();
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/bots/{encoded_id}/events?offset={off}&limit={lim}"),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_bot_telemetry() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/bots/telemetry")
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn get_bot_permissions(agent_id: String) -> Result<serde_json::Value, String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/bots/{}/permissions", urlencoding::encode(&agent_id)),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn set_bot_permissions(
    agent_id: String,
    permissions: serde_json::Value,
) -> Result<(), String> {
    validate_id(&agent_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json_no_content(
            &base_url,
            &format!("/api/v1/bots/{}/permissions", urlencoding::encode(&agent_id)),
            permissions,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn bot_workspace_list_files(
    bot_id: String,
    path: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let url = match &path {
            Some(p) => format!(
                "/api/v1/bots/{}/workspace/files?path={}",
                urlencoding::encode(&bot_id),
                urlencoding::encode(p),
            ),
            None => format!("/api/v1/bots/{}/workspace/files", urlencoding::encode(&bot_id)),
        };
        blocking_get_json::<serde_json::Value>(&base_url, &url)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn bot_workspace_read_file(
    bot_id: String,
    path: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/bots/{}/workspace/file?path={}",
                urlencoding::encode(&bot_id),
                urlencoding::encode(&path),
            ),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn bot_interaction(
    agent_id: String,
    response: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!("/api/v1/bots/{}/interaction", urlencoding::encode(&agent_id)),
            response,
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Generic daemon HTTP proxy — lets the frontend call any daemon API
/// endpoint via Tauri IPC, avoiding mixed-content blocks in production
/// builds where the webview origin is `https://tauri.localhost`.
#[tauri::command(rename_all = "snake_case")]
async fn daemon_fetch(
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let url = format!("{base_url}{path}");
        let c = client()?;
        let send = |req: reqwest::blocking::RequestBuilder| -> Result<serde_json::Value, String> {
            let resp = req.send().map_err(|e| e.to_string())?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
                // Retry once with a fresh token.
                invalidate_daemon_token();
                let c2 = client()?;
                let req2 = match method.as_str() {
                    "POST" => {
                        let r = with_auth(c2.post(&url));
                        match &body {
                            Some(b) => r.header("content-type", "application/json").json(b),
                            None => r,
                        }
                    }
                    "PUT" => {
                        let r = with_auth(c2.put(&url));
                        match &body {
                            Some(b) => r.header("content-type", "application/json").json(b),
                            None => r,
                        }
                    }
                    "DELETE" => with_auth(c2.delete(&url)),
                    _ => with_auth(c2.get(&url)),
                };
                let resp2 = req2.send().map_err(|e| e.to_string())?;
                return parse_response(resp2);
            }
            parse_response(resp)
        };

        let req = match method.as_str() {
            "POST" => {
                let r = with_auth(c.post(&url));
                match &body {
                    Some(b) => r.header("content-type", "application/json").json(b),
                    None => r,
                }
            }
            "PUT" => {
                let r = with_auth(c.put(&url));
                match &body {
                    Some(b) => r.header("content-type", "application/json").json(b),
                    None => r,
                }
            }
            "DELETE" => with_auth(c.delete(&url)),
            _ => with_auth(c.get(&url)),
        };

        send(req)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn parse_response(resp: reqwest::blocking::Response) -> Result<serde_json::Value, String> {
    let status = resp.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::Value::Null);
    }
    let text = resp.text().map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {text}"));
    }
    if text.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn blocking_get_json<T>(base_url: &str, path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    let response = with_auth(client()?.get(&url)).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.get(&url)).send().map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn blocking_post_empty<T>(base_url: &str, path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    let response = with_auth(client()?.post(&url)).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.post(&url)).send().map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn blocking_post_json<B, T>(base_url: &str, path: &str, body: B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    // Serialize body once so we can retry on 401 without requiring Clone.
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let response = with_auth(client()?.post(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.post(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn blocking_put_json<B, T>(base_url: &str, path: &str, body: B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let response = with_auth(client()?.put(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.put(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn blocking_patch_json<B, T>(base_url: &str, path: &str, body: B) -> Result<T, String>
where
    B: Serialize,
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let response = with_auth(client()?.patch(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.patch(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn blocking_delete<T>(base_url: &str, path: &str) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let url = format!("{base_url}{path}");
    let response = with_auth(client()?.delete(&url)).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response =
            with_auth(client()?.delete(&url)).send().map_err(|error| error.to_string())?;
        return parse_json(response);
    }
    parse_json(response)
}

fn parse_json<T>(response: Response) -> Result<T, String>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let body = response.text().map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {body}"));
    }

    let payload = if body.trim().is_empty() { "null" } else { body.as_str() };
    serde_json::from_str::<T>(payload).map_err(|error| error.to_string())
}

fn blocking_post_no_content(base_url: &str, path: &str) -> Result<(), String> {
    let url = format!("{base_url}{path}");
    let response = with_auth(client()?.post(&url)).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.post(&url)).send().map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!("{status}: {body}"));
        }
        return Ok(());
    }
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    Ok(())
}

fn blocking_post_json_no_content<B: Serialize>(
    base_url: &str,
    path: &str,
    body: B,
) -> Result<(), String> {
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let response = with_auth(client()?.post(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.post(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!("{status}: {body}"));
        }
        return Ok(());
    }
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    Ok(())
}

fn blocking_put_json_no_content<B: Serialize>(
    base_url: &str,
    path: &str,
    body: B,
) -> Result<(), String> {
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let response = with_auth(client()?.put(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response = with_auth(client()?.put(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!("{status}: {body}"));
        }
        return Ok(());
    }
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    Ok(())
}

fn blocking_delete_no_content(base_url: &str, path: &str) -> Result<(), String> {
    let url = format!("{base_url}{path}");
    let response = with_auth(client()?.delete(&url)).send().map_err(|error| error.to_string())?;
    if response.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let response =
            with_auth(client()?.delete(&url)).send().map_err(|error| error.to_string())?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!("{status}: {body}"));
        }
        return Ok(());
    }
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(format!("{status}: {body}"));
    }
    Ok(())
}

use std::sync::OnceLock;

fn shared_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client")
    })
}

/// Shared async HTTP client for SSE stream handlers and workflow commands.
fn shared_async_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(300))
            .pool_max_idle_per_host(5)
            .build()
            .expect("failed to build async reqwest client")
    })
}

fn client() -> Result<Client, String> {
    Ok(shared_client().clone())
}

// ── Daemon auth token cache ─────────────────────────────────────────
//
// The daemon generates a fresh auth token on every start and persists
// it to the OS keyring.  We cache it here so that we don't hit the
// keyring on every HTTP request.  When the daemon restarts (new token),
// requests return 401; we then invalidate the cache, re-read the
// keyring, and retry once.

static DAEMON_TOKEN: std::sync::LazyLock<Mutex<Option<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

/// Return the cached daemon auth token, reading from the OS keyring on
/// first call (or after invalidation).
fn get_daemon_token() -> Option<String> {
    let mut guard = DAEMON_TOKEN.lock().unwrap();
    if guard.is_none() {
        // Use load_direct() to read only the daemon:auth-token entry
        // instead of loading all secrets from the keyring.  This avoids
        // triggering keychain permission prompts for every stored secret.
        *guard = hive_core::daemon_token::load_direct();
        if guard.is_none() {
            tracing::warn!("unable to load daemon auth token from OS keyring");
        }
    }
    guard.clone()
}

/// Clear the cached token so the next `get_daemon_token` call re-reads
/// from the OS keyring.  Called when we receive a 401 from the daemon
/// (likely means the daemon restarted with a new token).
fn invalidate_daemon_token() {
    let mut guard = DAEMON_TOKEN.lock().unwrap();
    *guard = None;
}

/// Attach the daemon auth `Bearer` token to a request builder.
fn with_auth(builder: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
    match get_daemon_token() {
        Some(token) => builder.header("Authorization", format!("Bearer {token}")),
        None => builder,
    }
}

/// Same as [`with_auth`] but for the async reqwest client.
fn with_auth_async(builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match get_daemon_token() {
        Some(token) => builder.header("Authorization", format!("Bearer {token}")),
        None => builder,
    }
}

fn resolve_workspace_path(session_id: &str) -> Result<PathBuf, String> {
    validate_id(session_id)?;
    let base_url = daemon_url(None).map_err(|error| error.to_string())?;
    let session = blocking_get_json::<ChatSessionSnapshot>(
        &base_url,
        &format!("/api/v1/chat/sessions/{session_id}"),
    )?;
    let workspace_path = PathBuf::from(session.workspace_path);
    std::fs::create_dir_all(&workspace_path).map_err(|error| error.to_string())?;
    workspace_path.canonicalize().map_err(|error| error.to_string())
}

fn normalize_workspace_target_path(path: &str) -> Result<PathBuf, String> {
    if path.is_empty() || path == "." {
        return Ok(PathBuf::new());
    }

    let normalized = path.replace('\\', "/");
    let candidate = PathBuf::from(normalized);
    if candidate.components().all(|component| matches!(component, Component::Normal(_))) {
        Ok(candidate)
    } else {
        Err("Path traversal not allowed".to_string())
    }
}

fn ensure_path_within_workspace(path: &Path, workspace_root: &Path) -> Result<(), String> {
    let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
    if canonical_path.starts_with(workspace_root) {
        Ok(())
    } else {
        Err("Path traversal not allowed".to_string())
    }
}

fn remove_existing_destination(path: &Path, workspace_root: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }

    let canonical_path = path.canonicalize().map_err(|error| error.to_string())?;
    if canonical_path == workspace_root {
        return Err("cannot overwrite workspace root".to_string());
    }
    if !canonical_path.starts_with(workspace_root) {
        return Err("Path traversal not allowed".to_string());
    }

    let metadata = std::fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_dir() {
        std::fs::remove_dir_all(path).map_err(|error| error.to_string())
    } else {
        std::fs::remove_file(path).map_err(|error| error.to_string())
    }
}

fn copy_file_buffered(source: &Path, destination: &Path) -> Result<(), String> {
    let input = std::fs::File::open(source).map_err(|error| error.to_string())?;
    let output = std::fs::File::create(destination).map_err(|error| error.to_string())?;
    let mut reader = BufReader::new(input);
    let mut writer = BufWriter::new(output);
    std::io::copy(&mut reader, &mut writer).map_err(|error| error.to_string())?;
    Ok(())
}

#[allow(dead_code)]
fn copy_path_into_workspace(
    source: &Path,
    destination: &Path,
    workspace_root: &Path,
) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err(format!("clipboard paste does not support symlinks: {}", source.display()));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| format!("invalid destination path: {}", destination.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    ensure_path_within_workspace(parent, workspace_root)?;

    if metadata.is_dir() {
        remove_existing_destination(destination, workspace_root)?;
        std::fs::create_dir_all(destination).map_err(|error| error.to_string())?;
        ensure_path_within_workspace(destination, workspace_root)?;

        for entry in std::fs::read_dir(source).map_err(|error| error.to_string())? {
            let entry = entry.map_err(|error| error.to_string())?;
            copy_path_into_workspace(
                &entry.path(),
                &destination.join(entry.file_name()),
                workspace_root,
            )?;
        }
        return Ok(());
    }

    if !metadata.is_file() {
        return Err(format!(
            "clipboard paste only supports files and directories: {}",
            source.display()
        ));
    }

    remove_existing_destination(destination, workspace_root)?;
    copy_file_buffered(source, destination)
}

#[cfg(target_os = "macos")]
fn nsstring_from_str(
    value: &str,
) -> Result<objc2::rc::Retained<objc2_foundation::NSString>, String> {
    use std::ffi::CString;
    use std::ptr::NonNull;

    let c_string =
        CString::new(value).map_err(|_| "string contains interior NUL byte".to_string())?;
    let ptr = NonNull::new(c_string.as_ptr() as *mut std::ffi::c_char)
        .ok_or_else(|| "failed to create NSString pointer".to_string())?;
    unsafe { objc2_foundation::NSString::stringWithUTF8String(ptr) }
        .ok_or_else(|| "failed to convert string for clipboard".to_string())
}

#[cfg(target_os = "macos")]
fn nsstring_to_string(value: &objc2_foundation::NSString) -> Result<String, String> {
    let ptr = value.UTF8String();
    if ptr.is_null() {
        return Err("failed to read NSString contents".to_string());
    }
    Ok(unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy().into_owned())
}

#[cfg(target_os = "macos")]
fn clipboard_copy_files_macos(paths: Vec<String>) -> Result<(), String> {
    use objc2::runtime::ProtocolObject;
    use objc2_app_kit::{NSPasteboard, NSPasteboardWriting};
    use objc2_foundation::{NSArray, NSURL};

    let mut objects = Vec::with_capacity(paths.len());
    for path in paths {
        let path_buf = PathBuf::from(&path);
        if !path_buf.is_absolute() {
            return Err(format!("clipboard copy paths must be absolute: {path}"));
        }
        let metadata = std::fs::metadata(&path_buf).map_err(|error| {
            format!("failed to inspect clipboard path {}: {error}", path_buf.display())
        })?;
        let ns_path = nsstring_from_str(&path)?;
        let url = NSURL::fileURLWithPath_isDirectory(&ns_path, metadata.is_dir());
        objects.push(ProtocolObject::<dyn NSPasteboardWriting>::from_retained(url));
    }

    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    let array = NSArray::from_retained_slice(&objects);
    if pasteboard.writeObjects(&array) {
        Ok(())
    } else {
        Err("failed to write file references to clipboard".to_string())
    }
}

#[cfg(target_os = "macos")]
fn clipboard_read_file_paths_macos() -> Result<Vec<String>, String> {
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeFileURL};

    let pasteboard = NSPasteboard::generalPasteboard();
    let Some(items) = pasteboard.pasteboardItems() else {
        return Ok(Vec::new());
    };

    let mut paths = Vec::new();
    for item in items.iter() {
        let Some(url_string) = item.stringForType(unsafe { NSPasteboardTypeFileURL }) else {
            continue;
        };
        let Some(url) = objc2_foundation::NSURL::URLWithString(&url_string) else {
            continue;
        };
        if !url.isFileURL() {
            continue;
        }
        let Some(path) = url.path() else {
            continue;
        };
        paths.push(nsstring_to_string(&path)?);
    }

    Ok(paths)
}

// ── Windows clipboard support ──────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn clipboard_copy_files_windows(paths: Vec<String>) -> Result<(), String> {
    use clipboard_win::{formats, Clipboard, Setter};

    for path in &paths {
        let p = PathBuf::from(path);
        if !p.is_absolute() {
            return Err(format!("clipboard copy paths must be absolute: {path}"));
        }
        if !p.exists() {
            return Err(format!("clipboard path does not exist: {path}"));
        }
    }

    let _clip =
        Clipboard::new_attempts(10).map_err(|e| format!("failed to open clipboard: {e}"))?;

    formats::FileList
        .write_clipboard(&paths)
        .map_err(|e| format!("failed to write files to clipboard: {e}"))
}

#[cfg(target_os = "windows")]
fn clipboard_read_file_paths_windows() -> Result<Vec<String>, String> {
    use clipboard_win::{formats, Clipboard, Getter};

    let _clip =
        Clipboard::new_attempts(10).map_err(|e| format!("failed to open clipboard: {e}"))?;

    let mut file_list = Vec::new();
    formats::FileList
        .read_clipboard(&mut file_list)
        .map_err(|e| format!("failed to read files from clipboard: {e}"))?;

    Ok(file_list)
}

fn validate_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err("invalid id length".to_string());
    }
    // Allow `/` because agent IDs inherit the persona namespace (e.g. `system/general-a1b2c3d4`).
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/') {
        return Err(
            "id contains invalid characters (only alphanumeric, dash, underscore, slash allowed)"
                .to_string(),
        );
    }
    Ok(())
}

/// Like `validate_id` but also allows `/` for namespaced persona IDs (e.g. `system/general`).
fn validate_persona_id(id: &str) -> Result<(), String> {
    if id.is_empty() || id.len() > 128 {
        return Err("invalid persona id length".to_string());
    }
    if !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/') {
        return Err("persona id contains invalid characters (only alphanumeric, dash, underscore, slash allowed)"
            .to_string());
    }
    Ok(())
}

fn encode_query(query: &str) -> String {
    urlencoding::encode(query).into_owned()
}

#[tauri::command(rename_all = "snake_case")]
async fn github_auth_status() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/auth/github/status")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn github_list_models() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/auth/github/models")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn github_disconnect() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(&base_url, "/api/v1/auth/github/disconnect")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn github_start_device_flow() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(&base_url, "/api/v1/auth/github/device-code")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn github_poll_token(device_code: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<_, serde_json::Value>(
            &base_url,
            "/api/v1/auth/github/poll",
            serde_json::json!({ "device_code": device_code }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn github_save_token(
    provider_id: String,
    token: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<_, serde_json::Value>(
            &base_url,
            "/api/v1/auth/github/save-token",
            serde_json::json!({ "provider_id": provider_id, "token": token }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_subscribe_index_status(
    app: tauri::AppHandle,
    session_id: String,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!(
        "{}/api/v1/chat/sessions/{}/workspace/index-status/stream",
        base_url,
        urlencoding::encode(&session_id)
    );

    // Cancel any existing subscription for this session
    if let Ok(mut streams) = INDEX_STATUS_STREAMS.lock() {
        if let Some(prev) = streams.remove(&session_id) {
            prev.abort();
        }
    }

    let sid = session_id.clone();
    let sid_for_cleanup = session_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let client = shared_async_client().clone();
        let mut response = match with_auth_async(client.get(&url)).send().await {
            Ok(r) => r,
            Err(_) => return,
        };
        // Retry once on 401
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            invalidate_daemon_token();
            response = match with_auth_async(client.get(&url)).send().await {
                Ok(r) => r,
                Err(_) => return,
            };
        }
        if !response.status().is_success() {
            return;
        }

        use tokio_stream::StreamExt;
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    trim_sse_buffer(&mut buffer);
                    while let Some(pos) = buffer.find("\n\n") {
                        let event_block = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();
                        for line in event_block.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                                    let _ = app.emit(
                                        "index:event",
                                        serde_json::json!({
                                            "session_id": sid,
                                            "event": event,
                                        }),
                                    );
                                }
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }

        if let Ok(mut streams) = INDEX_STATUS_STREAMS.lock() {
            streams.remove(&sid_for_cleanup);
        }
    });

    if let Ok(mut streams) = INDEX_STATUS_STREAMS.lock() {
        streams.insert(session_id, handle);
    }

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_reindex_file(session_id: String, path: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!(
        "{}/api/v1/chat/sessions/{}/workspace/reindex?path={}",
        base_url,
        urlencoding::encode(&session_id),
        urlencoding::encode(&path)
    );
    let client = shared_async_client();
    let resp = with_auth_async(client.post(&url)).send().await.map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(client.post(&url)).send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("reindex failed: {}", resp.status()));
        }
        return Ok(());
    }
    if !resp.status().is_success() {
        return Err(format!("reindex failed: {}", resp.status()));
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_indexed_files(session_id: String) -> Result<Vec<String>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!(
        "{}/api/v1/chat/sessions/{}/workspace/index-status",
        base_url,
        urlencoding::encode(&session_id)
    );
    let client = shared_async_client();
    let resp = with_auth_async(client.get(&url)).send().await.map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(client.get(&url)).send().await.map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

#[tauri::command(rename_all = "snake_case")]
async fn chat_subscribe_stream(app: tauri::AppHandle, session_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url =
        format!("{}/api/v1/chat/sessions/{}/stream", base_url, urlencoding::encode(&session_id));

    // Cancel any existing stream task for this session
    if let Ok(mut streams) = ACTIVE_STREAMS.lock() {
        if let Some(prev) = streams.remove(&session_id) {
            prev.abort();
        }
    }

    let sid = session_id.clone();
    let sid_for_cleanup = session_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        let client = shared_async_client().clone();
        let mut response = match with_auth_async(client.get(&url)).send().await {
            Ok(r) => r,
            Err(e) => {
                let _ = app.emit(
                    "chat:error",
                    serde_json::json!({
                        "session_id": sid,
                        "kind": "transport",
                        "error": e.to_string(),
                    }),
                );
                return;
            }
        };

        // Retry once on 401
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            invalidate_daemon_token();
            response = match with_auth_async(client.get(&url)).send().await {
                Ok(r) => r,
                Err(e) => {
                    let _ = app.emit(
                        "chat:error",
                        serde_json::json!({
                            "session_id": sid,
                            "kind": "transport",
                            "error": e.to_string(),
                        }),
                    );
                    return;
                }
            };
        }

        if !response.status().is_success() {
            let status = response.status();
            let body = match response.text().await {
                Ok(body) => body,
                Err(e) => format!("<failed to read body: {e}>"),
            };
            let _ = app.emit(
                "chat:error",
                serde_json::json!({
                    "session_id": sid,
                    "kind": "http",
                    "error": format!("{}: {}", status, body),
                }),
            );
            return;
        }

        use tokio_stream::StreamExt;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));
                    trim_sse_buffer(&mut buffer);

                    while let Some(pos) = buffer.find("\n\n") {
                        let event_block = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        for line in event_block.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                if data == "[DONE]" {
                                    let _ = app.emit(
                                        "chat:done",
                                        serde_json::json!({
                                            "session_id": sid,
                                        }),
                                    );
                                    return;
                                }
                                if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                                    let _ = app.emit(
                                        "chat:event",
                                        serde_json::json!({
                                            "session_id": sid,
                                            "event": event,
                                        }),
                                    );
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = app.emit(
                        "chat:error",
                        serde_json::json!({
                            "session_id": sid,
                            "kind": "stream",
                            "error": e.to_string(),
                        }),
                    );
                    break;
                }
            }
        }
        // Clean up tracking entry when stream ends
        if let Ok(mut streams) = ACTIVE_STREAMS.lock() {
            streams.remove(&sid_for_cleanup);
        }
    });

    // Track the abort handle so we can cancel if a new subscription arrives
    if let Ok(mut streams) = ACTIVE_STREAMS.lock() {
        streams.insert(session_id, handle);
    }

    Ok(())
}

// ── Knowledge Graph commands ─────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn kg_get_neighbors(node_id: i64, limit: Option<usize>) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let lim = limit.unwrap_or(20);
    async_get_json(&base_url, &format!("/api/v1/knowledge/nodes/{node_id}/neighbors?limit={lim}"))
        .await
}

#[tauri::command(rename_all = "snake_case")]
async fn kg_vector_search(
    q: String,
    limit: Option<usize>,
    data_class: Option<String>,
    model: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let lim = limit.unwrap_or(20);
    let mut path =
        format!("/api/v1/knowledge/search/vector?q={}&limit={}", urlencoding::encode(&q), lim);
    if let Some(dc) = data_class {
        path.push_str(&format!("&data_class={}", urlencoding::encode(&dc)));
    }
    if let Some(m) = model {
        path.push_str(&format!("&model={}", urlencoding::encode(&m)));
    }
    async_get_json(&base_url, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_search_files(
    q: String,
    limit: Option<usize>,
    data_class: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let lim = limit.unwrap_or(20);
    let mut path =
        format!("/api/v1/knowledge/search/workspace?q={}&limit={}", urlencoding::encode(&q), lim);
    if let Some(dc) = data_class {
        path.push_str(&format!("&data_class={}", urlencoding::encode(&dc)));
    }
    async_get_json(&base_url, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn workspace_semantic_search(
    q: String,
    limit: Option<usize>,
    data_class: Option<String>,
    model: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let lim = limit.unwrap_or(20);
    let mut path = format!(
        "/api/v1/knowledge/search/workspace/semantic?q={}&limit={}",
        urlencoding::encode(&q),
        lim
    );
    if let Some(dc) = data_class {
        path.push_str(&format!("&data_class={}", urlencoding::encode(&dc)));
    }
    if let Some(m) = model {
        path.push_str(&format!("&model={}", urlencoding::encode(&m)));
    }
    async_get_json(&base_url, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn kg_list_embedding_models() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_get_json(&base_url, "/api/v1/knowledge/embedding-models").await
}

#[tauri::command(rename_all = "snake_case")]
async fn kg_update_node(
    node_id: i64,
    name: Option<String>,
    content: Option<String>,
    data_class: Option<String>,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let body = serde_json::json!({
        "name": name,
        "content": content,
        "data_class": data_class,
    });
    let url = format!("{base_url}/api/v1/knowledge/nodes/{node_id}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let client = shared_async_client();
    let resp = with_auth_async(client.put(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(client.put(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("update failed: {}", resp.status()));
        }
        return Ok(());
    }
    if !resp.status().is_success() {
        return Err(format!("update failed: {}", resp.status()));
    }
    Ok(())
}

// ── Daemon auth token (exposed to JS) ──────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn get_daemon_auth_token() -> Result<Option<String>, String> {
    Ok(get_daemon_token())
}

#[tauri::command(rename_all = "snake_case")]
fn invalidate_daemon_auth_token() -> Result<(), String> {
    invalidate_daemon_token();
    Ok(())
}

// ── Keyring (secure secret storage — via daemon API) ────────────────

#[tauri::command(rename_all = "snake_case")]
async fn save_secret(app: tauri::AppHandle, key: String, value: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!("/api/v1/secrets/{}", urlencoding::encode(&key));
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json_no_content::<serde_json::Value>(
            &base_url,
            &path,
            serde_json::json!({ "value": value }),
        )
    })
    .await
    .map_err(|e| e.to_string())??;
    let _ = app.emit("config:changed", "secrets");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn load_secret(key: String) -> Result<Option<String>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!("/api/v1/secrets/{}", urlencoding::encode(&key));
    let resp: serde_json::Value = tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, &path)
    })
    .await
    .map_err(|e| e.to_string())??;
    Ok(resp.get("value").and_then(|v| v.as_str()).map(|s| s.to_string()))
}

#[tauri::command(rename_all = "snake_case")]
async fn delete_secret(app: tauri::AppHandle, key: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!("/api/v1/secrets/{}", urlencoding::encode(&key));
    tauri::async_runtime::spawn_blocking(move || blocking_delete_no_content(&base_url, &path))
        .await
        .map_err(|e| e.to_string())??;
    let _ = app.emit("config:changed", "secrets");
    Ok(())
}

// ── Provider model discovery ──────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn fetch_provider_models(
    base_url: String,
    api_key: String,
    provider_kind: String,
    _api_version: Option<String>,
) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;
        match provider_kind.as_str() {
            "anthropic" => {
                let mut all_models = Vec::new();
                let mut after_id: Option<String> = None;
                for _ in 0..10 {
                    let mut url = format!("{}/v1/models?limit=100", base_url.trim_end_matches('/'));
                    if let Some(ref aid) = after_id {
                        url.push_str(&format!("&after_id={aid}"));
                    }
                    let resp = client
                        .get(&url)
                        .header("x-api-key", &api_key)
                        .header("anthropic-version", "2023-06-01")
                        .send()
                        .map_err(|e| format!("request failed: {e}"))?;
                    if !resp.status().is_success() {
                        let status = resp.status();
                        let body = resp.text().unwrap_or_default();
                        return Err(format!("{status}: {}", &body[..body.len().min(300)]));
                    }
                    let data: serde_json::Value =
                        resp.json().map_err(|e| format!("invalid JSON: {e}"))?;
                    if let Some(arr) = data.get("data").and_then(|d| d.as_array()) {
                        for m in arr {
                            if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                                all_models.push(id.to_string());
                            }
                        }
                    }
                    let has_more = data.get("has_more").and_then(|v| v.as_bool()).unwrap_or(false);
                    if !has_more {
                        break;
                    }
                    after_id = data.get("last_id").and_then(|v| v.as_str()).map(|s| s.to_string());
                }
                Ok(all_models)
            }
            "open-ai-compatible" => {
                let url = format!("{}/models", base_url.trim_end_matches('/'));
                let resp = client
                    .get(&url)
                    .header("Authorization", format!("Bearer {api_key}"))
                    .send()
                    .map_err(|e| format!("request failed: {e}"))?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().unwrap_or_default();
                    return Err(format!("{status}: {}", &body[..body.len().min(300)]));
                }
                let data: serde_json::Value =
                    resp.json().map_err(|e| format!("invalid JSON: {e}"))?;
                let mut all_models = Vec::new();
                if let Some(arr) = data.get("data").and_then(|d| d.as_array()) {
                    for m in arr {
                        if let Some(id) = m.get("id").and_then(|v| v.as_str()) {
                            all_models.push(id.to_string());
                        }
                    }
                }
                all_models.sort();
                Ok(all_models)
            }
            "microsoft-foundry" => {
                // Azure AI Foundry project API uses /deployments with api-version=v1
                let url = format!("{}/deployments?api-version=v1", base_url.trim_end_matches('/'));
                let resp = client
                    .get(&url)
                    .header("api-key", &api_key)
                    .send()
                    .map_err(|e| format!("request failed: {e}"))?;
                if !resp.status().is_success() {
                    let status = resp.status();
                    let body = resp.text().unwrap_or_default();
                    return Err(format!("{status}: {}", &body[..body.len().min(300)]));
                }
                let data: serde_json::Value =
                    resp.json().map_err(|e| format!("invalid JSON: {e}"))?;
                let mut all_models = Vec::new();
                // Response has { "value": [ { "name": "deployment-name", "modelName": "gpt-4o", ... } ] }
                if let Some(arr) = data.get("value").and_then(|d| d.as_array()) {
                    for m in arr {
                        if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
                            all_models.push(name.to_string());
                        }
                    }
                }
                all_models.sort();
                Ok(all_models)
            }
            _ => Err(format!("model discovery not supported for provider kind: {provider_kind}")),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Skills commands ─────────────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn skills_discover(persona_id: Option<String>) -> Result<Vec<DiscoveredSkill>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = match &persona_id {
            Some(pid) => format!("/api/v1/skills/discover?persona_id={}", urlencoding::encode(pid)),
            None => "/api/v1/skills/discover".to_string(),
        };
        blocking_post_empty::<Vec<DiscoveredSkill>>(&base_url, &path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_get_sources() -> Result<Vec<SkillSourceConfig>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<SkillSourceConfig>>(&base_url, "/api/v1/skills/sources")
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Response type for the lookup_model_metadata command.
#[derive(Serialize, Clone)]
struct ModelMetadataResponse {
    context_window: u32,
    max_output_tokens: u32,
    capabilities: Vec<String>,
}

/// Look up known metadata (context window, output limits, capabilities) for
/// one or more model names from the embedded model metadata registry.
#[tauri::command(rename_all = "snake_case")]
fn lookup_model_metadata(model_names: Vec<String>) -> HashMap<String, ModelMetadataResponse> {
    let registry = hive_core::ModelMetadataRegistry::load();
    model_names
        .into_iter()
        .map(|name| {
            let meta = registry.lookup(&name);
            let caps: Vec<String> = meta
                .capabilities
                .iter()
                .map(|c| {
                    // Serialize to the kebab-case form used in config/UI
                    serde_json::to_value(c)
                        .ok()
                        .and_then(|v| v.as_str().map(String::from))
                        .unwrap_or_default()
                })
                .collect();
            (
                name,
                ModelMetadataResponse {
                    context_window: meta.context_window,
                    max_output_tokens: meta.max_output_tokens,
                    capabilities: caps,
                },
            )
        })
        .collect()
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_set_sources(
    app: tauri::AppHandle,
    sources: Vec<SkillSourceConfig>,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<Vec<SkillSourceConfig>, serde_json::Value>(
            &base_url,
            "/api/v1/skills/sources",
            sources,
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "skills");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn list_personas(include_archived: Option<bool>) -> Result<Vec<Persona>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = if include_archived.unwrap_or(false) {
            "/api/v1/config/personas?include_archived=true".to_string()
        } else {
            "/api/v1/config/personas".to_string()
        };
        blocking_get_json::<Vec<Persona>>(&base_url, &path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_connectors() -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/config/connectors")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn list_connector_channels(connector_id: String) -> Result<Vec<serde_json::Value>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<serde_json::Value>>(
            &base_url,
            &format!("/api/v1/config/connectors/{}/channels", urlencoding::encode(&connector_id)),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn save_connectors(configs: Vec<serde_json::Value>) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<_, serde_json::Value>(&base_url, "/api/v1/config/connectors", configs)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn test_connector(
    connector_id: String,
    config: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = format!("/api/v1/config/connectors/{}/test", urlencoding::encode(&connector_id));
        match config {
            Some(body) => blocking_post_json::<_, serde_json::Value>(&base_url, &path, body),
            None => blocking_post_empty::<serde_json::Value>(&base_url, &path),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn connector_oauth_start(
    connector_id: String,
    provider: String,
    email: Option<String>,
    services: Option<Vec<String>>,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path =
            format!("/api/v1/config/connectors/{}/oauth/start", urlencoding::encode(&connector_id));
        blocking_post_json::<_, serde_json::Value>(
            &base_url,
            &path,
            serde_json::json!({
                "provider": provider,
                "email": email,
                "services": services,
                "client_id": client_id,
                "client_secret": client_secret,
            }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn connector_oauth_poll(
    connector_id: String,
    flow: String,
    device_code: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path =
            format!("/api/v1/config/connectors/{}/oauth/poll", urlencoding::encode(&connector_id));
        let mut body = serde_json::json!({ "flow": flow });
        if let Some(dc) = device_code {
            body["device_code"] = serde_json::Value::String(dc);
        }
        blocking_post_json::<_, serde_json::Value>(&base_url, &path, body)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn connector_discover(
    connector_id: String,
    provider_type: String,
    bot_token: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path =
            format!("/api/v1/config/connectors/{}/discover", urlencoding::encode(&connector_id));
        blocking_post_json::<_, serde_json::Value>(
            &base_url,
            &path,
            serde_json::json!({ "type": provider_type, "bot_token": bot_token }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Spawn the daemon binarywith --request-calendar-access / --request-contacts-access
/// to trigger macOS TCC permission prompts.  The HiveMind OS desktop app is the
/// "responsible" process, so macOS will allow the prompt to appear.
/// Returns { status: "granted"|"denied"|"error", detail: "..." }.
#[tauri::command(rename_all = "snake_case")]
async fn request_apple_access(
    calendar: bool,
    contacts: bool,
) -> Result<hive_core::AppleAccessResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        hive_core::request_apple_access(calendar, contacts)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[tauri::command(rename_all = "snake_case")]
async fn save_personas(app: tauri::AppHandle, personas: Vec<Persona>) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<Vec<Persona>, serde_json::Value>(
            &base_url,
            "/api/v1/config/personas",
            personas,
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "config");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn reset_persona(app: tauri::AppHandle, id: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = async_post_empty::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/config/personas/{}/reset", urlencoding::encode(&id)),
    )
    .await;
    if result.is_ok() {
        let _ = app.emit("config:changed", "config");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_rebuild_index() -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(&base_url, "/api/v1/skills/rebuild-index")
            .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_audit(
    app: tauri::AppHandle,
    source_id: String,
    source_path: String,
    model: String,
) -> Result<SkillAuditResult, String> {
    use tokio_stream::StreamExt;

    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let name = source_path.rsplit('/').next().unwrap_or(&source_path).to_string();
    let url = format!("{base_url}/api/v1/skills/{}/audit/stream", urlencoding::encode(&name));

    let client = shared_async_client();
    let body = serde_json::json!({
        "model": model,
        "source_id": source_id,
        "source_path": source_path,
    });

    let response = with_auth_async(client.post(&url))
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Could not reach the daemon: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        return Err(format!("{status}: {text}"));
    }

    let mut stream = response.bytes_stream();
    let mut buffer = String::new();
    let mut final_result: Option<SkillAuditResult> = None;
    let mut last_error: Option<String> = None;

    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| format!("Stream error: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&bytes));
        trim_sse_buffer(&mut buffer);

        while let Some(pos) = buffer.find("\n\n") {
            let block = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();
            for line in block.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                        // Forward progress events to the frontend
                        let _ = app.emit("skill:audit", data);

                        if let Some(phase) = event.get("phase").and_then(|v| v.as_str()) {
                            match phase {
                                "done" => {
                                    if let Some(result) = event.get("result") {
                                        final_result = serde_json::from_value(result.clone()).ok();
                                    }
                                }
                                "error" => {
                                    last_error = event
                                        .get("message")
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(result) = final_result {
        Ok(result)
    } else if let Some(err) = last_error {
        Err(err)
    } else {
        Err("Audit stream ended without a result".to_string())
    }
}

// ── Per-persona skill commands ──────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn copy_persona(
    app: tauri::AppHandle,
    source_id: String,
    new_id: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/config/personas/copy",
            serde_json::json!({ "source_id": source_id, "new_id": new_id }),
        )
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "config");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_list_installed_for_persona(
    persona_id: String,
) -> Result<Vec<InstalledSkill>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<Vec<InstalledSkill>>(
            &base_url,
            &format!("/api/v1/personas/{}/skills", urlencoding::encode(&persona_id)),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_install_for_persona(
    app: tauri::AppHandle,
    persona_id: String,
    name: String,
    source_id: String,
    source_path: String,
    model: String,
    audit: SkillAuditResult,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/personas/{}/skills/{}/install",
                urlencoding::encode(&persona_id),
                urlencoding::encode(&name),
            ),
            serde_json::json!({ "source_id": source_id, "source_path": source_path, "model": model, "audit": audit }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "skills");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_uninstall_for_persona(
    app: tauri::AppHandle,
    persona_id: String,
    name: String,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_delete::<serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/personas/{}/skills/{}",
                urlencoding::encode(&persona_id),
                urlencoding::encode(&name),
            ),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "skills");
    }
    result
}

#[tauri::command(rename_all = "snake_case")]
async fn skills_set_enabled_for_persona(
    app: tauri::AppHandle,
    persona_id: String,
    name: String,
    enabled: bool,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        blocking_put_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            &format!(
                "/api/v1/personas/{}/skills/{}/enabled",
                urlencoding::encode(&persona_id),
                urlencoding::encode(&name),
            ),
            serde_json::json!({ "enabled": enabled }),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?;
    if result.is_ok() {
        let _ = app.emit("config:changed", "skills");
    }
    result
}

// ── Event recording Tauri commands ──────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn event_recording_start(
    name: Option<String>,
    topic_filter: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/events/recordings",
            serde_json::json!({ "name": name, "topic_filter": topic_filter }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn event_recording_stop(recording_id: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/events/recordings/{recording_id}/stop"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn event_recording_list() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/events/recordings")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn event_recording_export(
    recording_id: String,
    format: Option<String>,
) -> Result<String, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let fmt = format.unwrap_or_else(|| "json".to_string());
    tauri::async_runtime::spawn_blocking(move || {
        let url = format!("{base_url}/api/v1/events/recordings/{recording_id}/export?format={fmt}");
        let response = with_auth(client()?.get(&url)).send().map_err(|e| e.to_string())?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            invalidate_daemon_token();
            let response = with_auth(client()?.get(&url)).send().map_err(|e| e.to_string())?;
            if !response.status().is_success() {
                return Err(format!("export failed: {}", response.status()));
            }
            return response.text().map_err(|e| e.to_string());
        }
        if !response.status().is_success() {
            return Err(format!("export failed: {}", response.status()));
        }
        response.text().map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn event_recording_delete(recording_id: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_delete::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/events/recordings/{recording_id}"),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------------------------------------------------------------------------
// Async HTTP helpers for workflow commands (avoids reqwest::blocking inside
// tokio spawn_blocking which can deadlock due to nested runtime conflicts)
// ---------------------------------------------------------------------------

async fn async_get_json<T: DeserializeOwned>(base_url: &str, path: &str) -> Result<T, String> {
    let url = format!("{base_url}{path}");
    let resp =
        with_auth_async(shared_async_client().get(&url)).send().await.map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(shared_async_client().get(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

async fn async_post_json<B: Serialize, T: DeserializeOwned>(
    base_url: &str,
    path: &str,
    body: B,
) -> Result<T, String> {
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let resp = with_auth_async(shared_async_client().post(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(shared_async_client().post(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

async fn async_post_empty<T: DeserializeOwned>(base_url: &str, path: &str) -> Result<T, String> {
    let url = format!("{base_url}{path}");
    let resp = with_auth_async(shared_async_client().post(&url))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(shared_async_client().post(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

async fn async_put_json<B: Serialize, T: DeserializeOwned>(
    base_url: &str,
    path: &str,
    body: B,
) -> Result<T, String> {
    let url = format!("{base_url}{path}");
    let body_bytes = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
    let resp = with_auth_async(shared_async_client().put(&url))
        .header("content-type", "application/json")
        .body(body_bytes.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(shared_async_client().put(&url))
            .header("content-type", "application/json")
            .body(body_bytes)
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

async fn async_delete<T: DeserializeOwned>(base_url: &str, path: &str) -> Result<T, String> {
    let url = format!("{base_url}{path}");
    let resp = with_auth_async(shared_async_client().delete(&url))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        invalidate_daemon_token();
        let resp = with_auth_async(shared_async_client().delete(&url))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        return async_parse_json(resp).await;
    }
    async_parse_json(resp).await
}

async fn async_parse_json<T: DeserializeOwned>(resp: reqwest::Response) -> Result<T, String> {
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("{status}: {body}"));
    }
    let payload = if body.trim().is_empty() { "null" } else { body.as_str() };
    serde_json::from_str::<T>(payload).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Workflow commands (use async HTTP client directly — no spawn_blocking)
// ---------------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
async fn workflow_list_definitions(mode: Option<String>) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = match mode {
        Some(m) => format!("/api/v1/workflows/definitions?mode={}", urlencoding::encode(&m)),
        None => "/api/v1/workflows/definitions".to_string(),
    };
    async_get_json::<serde_json::Value>(&base_url, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_save_definition(yaml: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/workflows/definitions",
        serde_json::json!({ "yaml": yaml }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_copy_definition(
    source_name: String,
    source_version: Option<String>,
    new_name: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/workflows/definitions/copy",
        serde_json::json!({
            "source_name": source_name,
            "source_version": source_version,
            "new_name": new_name,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_get_definition(
    name: String,
    version: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = match version {
        Some(v) => format!(
            "/api/v1/workflows/definitions/{}/{}",
            urlencoding::encode(&name),
            urlencoding::encode(&v)
        ),
        None => format!("/api/v1/workflows/definitions/{}", urlencoding::encode(&name)),
    };
    async_get_json::<serde_json::Value>(&base_url, &path).await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_delete_definition(
    name: String,
    version: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_delete::<serde_json::Value>(
        &base_url,
        &format!(
            "/api/v1/workflows/definitions/{}/{}",
            urlencoding::encode(&name),
            urlencoding::encode(&version)
        ),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_reset_definition(name: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_empty::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/definitions/{}/reset", urlencoding::encode(&name)),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_archive_definition(
    name: String,
    version: String,
    archived: Option<bool>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &format!(
            "/api/v1/workflows/definitions/{}/{}/archive",
            urlencoding::encode(&name),
            urlencoding::encode(&version)
        ),
        serde_json::json!({ "archived": archived.unwrap_or(true) }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_archive_instance(
    instance_id: i64,
    archived: Option<bool>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}/archive"),
        serde_json::json!({ "archived": archived.unwrap_or(true) }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_set_triggers_paused(
    name: String,
    version: String,
    paused: Option<bool>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &format!(
            "/api/v1/workflows/definitions/{}/{}/triggers-paused",
            urlencoding::encode(&name),
            urlencoding::encode(&version)
        ),
        serde_json::json!({ "paused": paused.unwrap_or(true) }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_check_definition_dependents(
    name: String,
    version: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_get_json::<serde_json::Value>(
        &base_url,
        &format!(
            "/api/v1/workflows/definitions/{}/{}/dependents",
            urlencoding::encode(&name),
            urlencoding::encode(&version)
        ),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
#[allow(clippy::too_many_arguments)]
async fn workflow_launch(
    definition: String,
    version: Option<String>,
    inputs: serde_json::Value,
    parent_session_id: String,
    parent_agent_id: Option<String>,
    permissions: Option<serde_json::Value>,
    trigger_step_id: Option<String>,
    workspace_path: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/workflows/instances",
        serde_json::json!({
            "definition": definition,
            "version": version,
            "inputs": inputs,
            "parent_session_id": parent_session_id,
            "parent_agent_id": parent_agent_id,
            "permissions": permissions,
            "trigger_step_id": trigger_step_id,
            "workspace_path": workspace_path,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_list_instances(
    status: Option<String>,
    definition: Option<String>,
    session_id: Option<String>,
    agent_id: Option<String>,
    limit: Option<u32>,
    offset: Option<u32>,
    include_archived: Option<bool>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let mut params = Vec::new();
    if let Some(s) = &status {
        params.push(format!("status={}", urlencoding::encode(s)));
    }
    if let Some(d) = &definition {
        params.push(format!("definition={}", urlencoding::encode(d)));
    }
    if let Some(s) = &session_id {
        params.push(format!("session_id={}", urlencoding::encode(s)));
    }
    if let Some(a) = &agent_id {
        params.push(format!("agent_id={}", urlencoding::encode(a)));
    }
    if let Some(l) = limit {
        params.push(format!("limit={l}"));
    }
    if let Some(o) = offset {
        params.push(format!("offset={o}"));
    }
    if include_archived == Some(true) {
        params.push("include_archived=true".to_string());
    }
    let query = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
    async_get_json::<serde_json::Value>(&base_url, &format!("/api/v1/workflows/instances{query}"))
        .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_get_instance(instance_id: i64) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_get_json::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}"),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_pause(instance_id: i64) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_empty::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}/pause"),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_resume(instance_id: i64) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_empty::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}/resume"),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_kill(instance_id: i64) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_empty::<serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}/kill"),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_respond_gate(
    instance_id: i64,
    step_id: String,
    response: serde_json::Value,
) -> Result<serde_json::Value, String> {
    validate_id(&step_id)?;
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &format!(
            "/api/v1/workflows/instances/{}/steps/{}/respond",
            instance_id,
            urlencoding::encode(&step_id),
        ),
        serde_json::json!({ "response": response }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_update_permissions(
    instance_id: i64,
    permissions: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_put_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &format!("/api/v1/workflows/instances/{instance_id}/permissions"),
        permissions,
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_subscribe_events(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/workflows/events");

    // Cancel any existing subscription.
    if let Ok(mut guard) = WORKFLOW_EVENT_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            if let Ok(event) = serde_json::from_str::<serde_json::Value>(data) {
                let _ = app.emit("workflow:event", event);
            }
            true
        })
        .await;

        // Clean up when stream ends.
        if let Ok(mut guard) = WORKFLOW_EVENT_STREAM.lock() {
            *guard = None;
        }
    });

    if let Ok(mut guard) = WORKFLOW_EVENT_STREAM.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

// ── Workflow AI Assist ──────────────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn workflow_ai_assist(
    yaml: String,
    prompt: String,
    agent_id: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut body = serde_json::json!({ "yaml": yaml, "prompt": prompt });
        if let Some(id) = agent_id {
            body["agent_id"] = serde_json::Value::String(id);
        }
        blocking_post_json::<serde_json::Value, serde_json::Value>(
            &base_url,
            "/api/v1/workflows/ai-assist",
            body,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Agent Kit commands ──────────────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn agent_kit_export(
    kit_name: String,
    description: Option<String>,
    author: Option<String>,
    persona_ids: Vec<String>,
    workflow_names: Vec<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/agent-kits/export",
        serde_json::json!({
            "kit_name": kit_name,
            "description": description,
            "author": author,
            "persona_ids": persona_ids,
            "workflow_names": workflow_names,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_kit_preview(
    content: String,
    target_namespace: String,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/agent-kits/preview",
        serde_json::json!({
            "content": content,
            "target_namespace": target_namespace,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_kit_import(
    content: String,
    target_namespace: String,
    selected_items: Vec<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        "/api/v1/agent-kits/import",
        serde_json::json!({
            "content": content,
            "target_namespace": target_namespace,
            "selected_items": selected_items,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_kit_save_file(path: String, content: String) -> Result<(), String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&content)
        .map_err(|e| format!("invalid base64: {e}"))?;
    std::fs::write(&path, &bytes).map_err(|e| format!("failed to write file: {e}"))
}

#[tauri::command(rename_all = "snake_case")]
async fn agent_kit_read_file(path: String) -> Result<String, String> {
    use base64::Engine;
    let bytes = std::fs::read(&path).map_err(|e| format!("failed to read file: {e}"))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(&bytes))
}

// ── Flight Deck commands ────────────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn flight_deck_system_health() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/system/health")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn flight_deck_all_agents() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/agents")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn flight_deck_sessions_telemetry() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/chat/sessions/telemetry")
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Services dashboard commands ─────────────────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn services_list() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/services")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn services_get_logs(
    service_id: String,
    since_ms: Option<u64>,
    limit: Option<usize>,
    level: Option<String>,
    search: Option<String>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut params = Vec::new();
        if let Some(s) = since_ms {
            params.push(format!("since_ms={s}"));
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        if let Some(ref lv) = level {
            params.push(format!("level={}", encode_query(lv)));
        }
        if let Some(ref q) = search {
            params.push(format!("search={}", encode_query(q)));
        }
        let qs = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
        let path = format!("/api/v1/services/{}/logs{}", encode_query(&service_id), qs);
        blocking_get_json::<serde_json::Value>(&base_url, &path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn services_restart(service_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let path = format!("/api/v1/services/{}/restart", encode_query(&service_id));
        blocking_post_no_content(&base_url, &path)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn services_subscribe_events(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/services/events");

    tokio::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("service:event", data.to_string());
            true
        })
        .await;
    });

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn mcp_subscribe_events(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/mcp/events");

    // Cancel any existing subscription.
    if let Ok(mut guard) = MCP_EVENT_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("mcp:event", data.to_string());
            true
        })
        .await;

        // Clean up when stream ends.
        if let Ok(mut guard) = MCP_EVENT_STREAM.lock() {
            *guard = None;
        }
    });

    if let Ok(mut guard) = MCP_EVENT_STREAM.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn scheduler_subscribe_events(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/scheduler/events");

    tokio::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("scheduler:event", data.to_string());
            true
        })
        .await;
    });

    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn event_bus_subscribe(app: tauri::AppHandle) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let url = format!("{base_url}/api/v1/events/stream");

    // Cancel any existing subscription.
    if let Ok(mut guard) = EVENTBUS_STREAM.lock() {
        if let Some(prev) = guard.take() {
            prev.abort();
        }
    }

    let handle = tauri::async_runtime::spawn(async move {
        sse_subscribe_loop(url, true, move |data| {
            let _ = app.emit("eventbus:event", data.to_string());
            true
        })
        .await;

        // Clean up when stream ends.
        if let Ok(mut guard) = EVENTBUS_STREAM.lock() {
            *guard = None;
        }
    });

    if let Ok(mut guard) = EVENTBUS_STREAM.lock() {
        *guard = Some(handle);
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn event_bus_query(
    topic: Option<String>,
    since: Option<u64>,
    before_id: Option<i64>,
    after_id: Option<i64>,
    limit: Option<u32>,
) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let mut params = Vec::new();
        if let Some(t) = &topic {
            params.push(format!("topic={}", encode_query(t)));
        }
        if let Some(s) = since {
            params.push(format!("since={s}"));
        }
        if let Some(b) = before_id {
            params.push(format!("before_id={b}"));
        }
        if let Some(a) = after_id {
            params.push(format!("after_id={a}"));
        }
        if let Some(l) = limit {
            params.push(format!("limit={l}"));
        }
        let qs = if params.is_empty() { String::new() } else { format!("?{}", params.join("&")) };
        blocking_get_json::<serde_json::Value>(&base_url, &format!("/api/v1/events{qs}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn event_bus_topics() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/workflows/topics")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_list_active_triggers() -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(&base_url, "/api/v1/workflows/triggers/active")
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_upload_attachment(
    workflow_id: String,
    version: String,
    file_path: String,
    description: String,
) -> Result<serde_json::Value, String> {
    use base64::Engine;

    let data =
        tokio::fs::read(&file_path).await.map_err(|e| format!("failed to read file: {e}"))?;
    let filename = std::path::Path::new(&file_path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unnamed".to_string());
    let media_type = mime_guess::from_path(&file_path).first().map(|m| m.to_string());
    let content = base64::engine::general_purpose::STANDARD.encode(&data);

    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!(
        "/api/v1/workflows/attachments/{}/{}",
        urlencoding::encode(&workflow_id),
        urlencoding::encode(&version),
    );
    async_post_json::<serde_json::Value, serde_json::Value>(
        &base_url,
        &path,
        serde_json::json!({
            "filename": filename,
            "description": description,
            "media_type": media_type,
            "content": content,
        }),
    )
    .await
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_delete_attachment(
    workflow_id: String,
    version: String,
    attachment_id: String,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!(
        "/api/v1/workflows/attachments/{}/{}/{}",
        urlencoding::encode(&workflow_id),
        urlencoding::encode(&version),
        urlencoding::encode(&attachment_id),
    );
    async_delete::<serde_json::Value>(&base_url, &path).await.map(|_| ())
}

#[tauri::command(rename_all = "snake_case")]
async fn workflow_copy_attachments(
    workflow_id: String,
    from_version: String,
    to_version: String,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    let path = format!(
        "/api/v1/workflows/attachments/{}/{}/copy/{}",
        urlencoding::encode(&workflow_id),
        urlencoding::encode(&from_version),
        urlencoding::encode(&to_version),
    );
    async_post_empty::<serde_json::Value>(&base_url, &path).await.map(|_| ())
}

#[tauri::command(rename_all = "snake_case")]
async fn local_model_load(model_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/local-models/{}/load", encode_query(&model_id)),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn local_model_unload(model_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_post_empty::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/local-models/{}/unload", encode_query(&model_id)),
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ── Plugin management commands ─────────────────────────────────────────────

#[derive(serde::Serialize)]
struct PluginInfo {
    plugin_id: String,
    name: String,
    version: String,
    display_name: String,
    description: String,
    plugin_type: String,
    enabled: bool,
    config: serde_json::Value,
    config_schema: Option<serde_json::Value>,
    status: Option<PluginStatusInfo>,
    permissions: Vec<String>,
}

#[derive(serde::Serialize)]
struct PluginStatusInfo {
    state: String,
    message: Option<String>,
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_list() -> Result<Vec<PluginInfo>, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        match blocking_get_json::<Vec<serde_json::Value>>(&base_url, "/api/v1/plugins") {
            Ok(items) => Ok(items
                .into_iter()
                .filter_map(|v| {
                    Some(PluginInfo {
                        plugin_id: v.get("plugin_id")?.as_str()?.to_string(),
                        name: v.get("name")?.as_str()?.to_string(),
                        version: v
                            .get("version")
                            .and_then(|x| x.as_str())
                            .unwrap_or("0.0.0")
                            .to_string(),
                        display_name: v
                            .get("display_name")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        description: v
                            .get("description")
                            .and_then(|x| x.as_str())
                            .unwrap_or("")
                            .to_string(),
                        plugin_type: v
                            .get("plugin_type")
                            .and_then(|x| x.as_str())
                            .unwrap_or("connector")
                            .to_string(),
                        enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
                        config: v
                            .get("config")
                            .cloned()
                            .unwrap_or(serde_json::Value::Object(Default::default())),
                        config_schema: v.get("config_schema").cloned().filter(|v| !v.is_null()),
                        status: v.get("status").and_then(|s| {
                            Some(PluginStatusInfo {
                                state: s.get("state")?.as_str()?.to_string(),
                                message: s
                                    .get("message")
                                    .and_then(|x| x.as_str())
                                    .map(|x| x.to_string()),
                            })
                        }),
                        permissions: v
                            .get("permissions")
                            .and_then(|x| x.as_array())
                            .map(|a| {
                                a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()
                            })
                            .unwrap_or_default(),
                    })
                })
                .collect()),
            Err(_) => Ok(Vec::new()),
        }
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_get_config_schema(plugin_id: String) -> Result<serde_json::Value, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        blocking_get_json::<serde_json::Value>(
            &base_url,
            &format!("/api/v1/plugins/{}/config-schema", encode_query(&plugin_id)),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_save_config(plugin_id: String, config: serde_json::Value) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        blocking_post_json_no_content(
            &base_url,
            &format!("/api/v1/plugins/{}/config", encode_query(&plugin_id)),
            &config,
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_set_enabled(plugin_id: String, enabled: bool) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        blocking_post_json_no_content(
            &base_url,
            &format!("/api/v1/plugins/{}/enabled", encode_query(&plugin_id)),
            &serde_json::json!({ "enabled": enabled }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_set_personas(
    plugin_id: String,
    allowed_personas: Vec<String>,
) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        blocking_post_json_no_content(
            &base_url,
            &format!("/api/v1/plugins/{}/personas", encode_query(&plugin_id)),
            &serde_json::json!({ "allowed_personas": allowed_personas }),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_uninstall(plugin_id: String) -> Result<(), String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        blocking_delete_no_content(
            &base_url,
            &format!("/api/v1/plugins/{}", encode_query(&plugin_id)),
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command(rename_all = "snake_case")]
async fn plugin_link_local(path: String) -> Result<String, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = serde_json::json!({ "path": path });
        blocking_post_json::<_, serde_json::Value>(&base_url, "/api/v1/plugins/link", &body)
            .map(|v| v.get("plugin_id").and_then(|x| x.as_str()).unwrap_or("").to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn plugin_install_npm(package_name: String) -> Result<String, String> {
    let base_url = daemon_url(None).map_err(|e| e.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        let body = serde_json::json!({ "package": package_name });
        blocking_post_json::<_, serde_json::Value>(&base_url, "/api/v1/plugins/install", &body)
            .map(|v| v.get("plugin_id").and_then(|x| x.as_str()).unwrap_or("").to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState {
            paste_cancelled: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_installing: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            paste_conflict_tx: Mutex::new(None),
            paste_conflict_rx: Mutex::new(None),
            paste_conflict_response_tx: Mutex::new(None),
            paste_conflict_response_rx: Mutex::new(None),
        })
        .setup(|app| {
            app.handle().plugin(tauri_plugin_dialog::init())?;
            app.handle().plugin(tauri_plugin_process::init())?;
            // Only enable the auto-updater when a signing pubkey is configured;
            // an empty key would cause signature verification failures.
            let updater_configured = app
                .config()
                .plugins
                .0
                .get("updater")
                .and_then(|v| v.get("pubkey"))
                .and_then(|v| v.as_str())
                .is_some_and(|k| !k.is_empty());
            if updater_configured {
                app.handle().plugin(tauri_plugin_updater::Builder::new().build())?;
            }
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default().level(log::LevelFilter::Info).build(),
                )?;
            }

            tray::setup_tray(app.handle())?;
            tray::setup_close_to_tray(app.handle());
            update::spawn_update_timer(app.handle());
            service_registration::ensure_daemon_service_registered();

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_url,
            daemon_status,
            daemon_start,
            daemon_stop,
            set_update_installing,
            config_show,
            config_get,
            config_save,
            app_context,
            canvas_ws_url,
            chat_list_sessions,
            chat_create_session,
            chat_get_session,
            chat_set_session_persona,
            chat_rename_session,
            list_session_agents,
            pause_session_agent,
            resume_session_agent,
            kill_session_agent,
            restart_session_agent,
            agent_stage_subscribe,
            get_agent_telemetry,
            get_agent_events,
            get_session_events,
            list_session_processes,
            kill_process,
            get_process_status,
            process_subscribe_events,
            agent_approve_tool,
            agent_respond_interaction,
            list_pending_approvals,
            list_session_pending_questions,
            list_all_pending_questions,
            list_pending_interactions,
            get_pending_interaction_counts,
            subscribe_approval_stream,
            interactions_subscribe,
            write_frontend_log,
            chat_delete_session,
            chat_send_message,
            chat_approve_tool,
            chat_respond_interaction,
            chat_interrupt,
            chat_resume,
            recluster_canvas,
            propose_layout,
            get_session_permissions,
            set_session_permissions,
            chat_get_session_memory,
            chat_upload_file,
            chat_link_workspace,
            workspace_list_files,
            workspace_read_file,
            workspace_save_file,
            workspace_create_directory,
            workspace_delete_entry,
            workspace_move_entry,
            clipboard_copy_files,
            clipboard_read_file_paths,
            clipboard_paste_files,
            clipboard_cancel_paste,
            clipboard_resolve_conflict,
            workspace_audit_file,
            workspace_get_audit,
            workspace_get_classification,
            workspace_set_classification_default,
            workspace_set_classification_override,
            workspace_clear_classification_override,
            memory_search,
            chat_list_risk_scans,
            model_router_snapshot,
            mcp_list_servers,
            mcp_connect_server,
            mcp_disconnect_server,
            mcp_list_tools,
            mcp_list_resources,
            mcp_list_prompts,
            mcp_list_notifications,
            mcp_server_logs,
            mcp_registry_search,
            tools_list,
            local_models_list,
            local_models_get,
            local_models_install,
            local_models_update_params,
            local_models_remove,
            local_models_search,
            local_models_hub_files,
            local_models_hardware,
            local_models_downloads,
            local_models_remove_download,
            local_models_resource_usage,
            local_models_storage,
            chat_subscribe_stream,
            workspace_subscribe_index_status,
            workspace_reindex_file,
            workspace_indexed_files,
            kg_get_neighbors,
            kg_vector_search,
            workspace_search_files,
            workspace_semantic_search,
            kg_list_embedding_models,
            kg_update_node,
            github_auth_status,
            github_list_models,
            github_disconnect,
            github_start_device_flow,
            github_poll_token,
            github_save_token,
            get_daemon_auth_token,
            invalidate_daemon_auth_token,
            daemon_fetch,
            save_secret,
            load_secret,
            delete_secret,
            fetch_provider_models,
            lookup_model_metadata,
            skills_discover,
            skills_get_sources,
            skills_set_sources,
            list_personas,
            list_connectors,
            list_connector_channels,
            save_connectors,
            test_connector,
            connector_oauth_start,
            connector_oauth_poll,
            connector_discover,
            request_apple_access,
            save_personas,
            reset_persona,
            skills_rebuild_index,
            skills_audit,
            copy_persona,
            skills_list_installed_for_persona,
            skills_install_for_persona,
            skills_uninstall_for_persona,
            skills_set_enabled_for_persona,
            list_bots,
            launch_bot,
            message_bot,
            render_prompt_template,
            send_prompt_to_bot,
            deactivate_bot,
            activate_bot,
            delete_bot,
            bot_subscribe,
            ensure_bot_stream,
            get_bot_events,
            get_bot_telemetry,
            bot_interaction,
            get_bot_permissions,
            set_bot_permissions,
            bot_workspace_list_files,
            bot_workspace_read_file,
            event_recording_start,
            event_recording_stop,
            event_recording_list,
            event_recording_export,
            event_recording_delete,
            workflow_list_definitions,
            workflow_save_definition,
            workflow_copy_definition,
            workflow_get_definition,
            workflow_delete_definition,
            workflow_reset_definition,
            workflow_archive_definition,
            workflow_set_triggers_paused,
            workflow_check_definition_dependents,
            workflow_launch,
            workflow_list_instances,
            workflow_get_instance,
            workflow_pause,
            workflow_resume,
            workflow_kill,
            workflow_archive_instance,
            workflow_respond_gate,
            workflow_update_permissions,
            workflow_subscribe_events,
            workflow_ai_assist,
            agent_kit_export,
            agent_kit_preview,
            agent_kit_import,
            agent_kit_save_file,
            agent_kit_read_file,
            flight_deck_system_health,
            flight_deck_all_agents,
            flight_deck_sessions_telemetry,
            event_bus_query,
            event_bus_topics,
            event_bus_subscribe,
            workflow_list_active_triggers,
            workflow_upload_attachment,
            workflow_delete_attachment,
            workflow_copy_attachments,
            local_model_load,
            local_model_unload,
            get_user_status,
            set_user_status,
            status_heartbeat,
            services_list,
            services_get_logs,
            services_restart,
            services_subscribe_events,
            mcp_subscribe_events,
            scheduler_subscribe_events,
            plugin_list,
            plugin_get_config_schema,
            plugin_save_config,
            plugin_set_enabled,
            plugin_set_personas,
            plugin_uninstall,
            plugin_link_local,
            plugin_install_npm,
        ])
        .build(tauri::generate_context!())
        .expect("error while building hivemind desktop")
        .run(|_app, _event| {
            // On macOS, clicking the dock icon when all windows are hidden
            // fires RunEvent::Reopen.  Show the main window so the user
            // can get back to the app without using the tray icon.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                if let Some(window) = _app.get_webview_window("main") {
                    let _ = window.show();
                    let _ = window.unminimize();
                    let _ = window.set_focus();
                }
            }
        });
}
