use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{mpsc, Notify};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tracing::{debug, error, info, warn};

use super::api;
use super::api::DiscordApiError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A parsed Discord MESSAGE_CREATE event with the fields we care about.
pub(crate) struct GatewayMessage {
    pub channel_id: String,
    pub guild_id: Option<String>,
    pub author_id: String,
    pub author_name: String,
    pub content: String,
    pub message_id: String,
    pub timestamp: String,
    /// If this message is a reply, the ID of the message being replied to.
    pub referenced_message_id: Option<String>,
    /// If this message is from a Discord interaction (button click).
    pub interaction: Option<DiscordInteraction>,
}

/// A parsed Discord component interaction (button click from AFK messages).
pub(crate) struct DiscordInteraction {
    pub custom_id: String,
    pub interaction_id: String,
    pub interaction_token: String,
}

/// State needed to RESUME a gateway session after a disconnect.
#[derive(Clone, Debug)]
struct ResumeState {
    session_id: String,
    resume_url: String,
    seq: Option<u64>,
}

// ---------------------------------------------------------------------------
// Public entry-point
// ---------------------------------------------------------------------------

/// Start the Discord Gateway connection as a background task.
///
/// Returns a `JoinHandle` for the gateway loop and an `mpsc::Receiver` that
/// yields incoming messages.  The `notify` is signalled after every message
/// so that `wait_for_changes` can wake up promptly.
pub(crate) async fn start_gateway(
    bot_token: String,
    allowed_guild_ids: Vec<String>,
    listen_channel_ids: Vec<String>,
    notify: Arc<Notify>,
) -> Result<(tokio::task::JoinHandle<()>, mpsc::Receiver<GatewayMessage>)> {
    let (tx, rx) = mpsc::channel::<GatewayMessage>(256);

    let handle = tokio::spawn(async move {
        gateway_loop(bot_token, allowed_guild_ids, listen_channel_ids, tx, notify).await;
    });

    Ok((handle, rx))
}

// ---------------------------------------------------------------------------
// Gateway loop (reconnects on failure)
// ---------------------------------------------------------------------------

async fn gateway_loop(
    bot_token: String,
    allowed_guild_ids: Vec<String>,
    listen_channel_ids: Vec<String>,
    tx: mpsc::Sender<GatewayMessage>,
    notify: Arc<Notify>,
) {
    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);
    let http = reqwest::Client::new();
    let mut resume_state: Option<ResumeState> = None;

    // Rate limiter: stop the gateway if too many reconnects happen without
    // establishing a stable session.
    let mut reconnect_times: VecDeque<Instant> = VecDeque::new();
    const MAX_RECONNECTS: usize = 10;
    const RATE_WINDOW: Duration = Duration::from_secs(300); // 5 minutes
    const STABLE_THRESHOLD: Duration = Duration::from_secs(30);

    // Circuit-breaker: stop after repeated auth failures (invalid token).
    let mut consecutive_auth_failures: u32 = 0;
    const MAX_AUTH_FAILURES: u32 = 3;

    loop {
        // Expire old entries and enforce rate limit
        let now = Instant::now();
        while reconnect_times.front().map_or(false, |t| now.duration_since(*t) > RATE_WINDOW) {
            reconnect_times.pop_front();
        }
        if reconnect_times.len() >= MAX_RECONNECTS {
            error!(
                "Discord gateway: {} reconnects in {:?} without a stable session, stopping",
                MAX_RECONNECTS, RATE_WINDOW
            );
            return;
        }

        let session_start = Instant::now();

        match run_session(
            &http,
            &bot_token,
            &allowed_guild_ids,
            &listen_channel_ids,
            &tx,
            &notify,
            resume_state.as_ref(),
        )
        .await
        {
            Ok(SessionExit::ReceiverDropped) => {
                info!("Discord gateway: receiver dropped, exiting");
                return;
            }
            Ok(SessionExit::Reconnect(state)) => {
                consecutive_auth_failures = 0;
                let was_stable = session_start.elapsed() >= STABLE_THRESHOLD;
                if was_stable {
                    backoff = Duration::from_secs(1);
                    reconnect_times.clear();
                }

                resume_state = state;
                if resume_state.is_some() {
                    info!("Discord gateway: reconnecting with RESUME");
                    tokio::time::sleep(backoff.max(Duration::from_secs(1))).await;
                } else {
                    info!("Discord gateway: reconnecting with fresh IDENTIFY");
                    tokio::time::sleep(backoff).await;
                }

                if !was_stable {
                    backoff = (backoff * 2).min(max_backoff);
                }
                reconnect_times.push_back(Instant::now());
            }
            Ok(SessionExit::InvalidSession) => {
                consecutive_auth_failures = 0;
                info!("Discord gateway: session invalidated, will re-IDENTIFY");
                resume_state = None;
                tokio::time::sleep(backoff.max(Duration::from_secs(3))).await;
                backoff = (backoff * 2).min(max_backoff);
                reconnect_times.push_back(Instant::now());
            }
            Err(e) => {
                // Check if the error chain contains an auth failure.
                let is_auth_failure = e.chain().any(|cause| {
                    cause
                        .downcast_ref::<DiscordApiError>()
                        .map_or(false, |de| matches!(de, DiscordApiError::AuthFailed(_)))
                });

                if is_auth_failure {
                    consecutive_auth_failures += 1;
                    if consecutive_auth_failures >= MAX_AUTH_FAILURES {
                        error!(
                            "Discord connector disabled — authentication failed after {} \
                             consecutive attempts. Please update credentials in connector settings.",
                            consecutive_auth_failures
                        );
                        return;
                    }
                    warn!(
                        consecutive_auth_failures,
                        "Discord gateway: authentication failed, will retry"
                    );
                } else {
                    consecutive_auth_failures = 0;
                }

                warn!(?e, ?backoff, "Discord gateway session error, will reconnect");
                if resume_state.is_some() {
                    info!("Resume connection failed, will try fresh IDENTIFY");
                    resume_state = None;
                }
                tokio::time::sleep(backoff).await;
                backoff = (backoff * 2).min(max_backoff);
                reconnect_times.push_back(Instant::now());
            }
        }
    }
}

enum SessionExit {
    /// Connection lost or Discord requested reconnect; carry resume state if available.
    Reconnect(Option<ResumeState>),
    /// Session was explicitly invalidated (op 9, not resumable); must re-IDENTIFY.
    InvalidSession,
    /// The message channel receiver was dropped; stop the gateway.
    ReceiverDropped,
}

// ---------------------------------------------------------------------------
// Single session
// ---------------------------------------------------------------------------

async fn run_session(
    http: &reqwest::Client,
    bot_token: &str,
    allowed_guild_ids: &[String],
    listen_channel_ids: &[String],
    tx: &mpsc::Sender<GatewayMessage>,
    notify: &Arc<Notify>,
    resume: Option<&ResumeState>,
) -> Result<SessionExit> {
    // 1. Build WebSocket URL — use resume URL if resuming, otherwise fetch fresh
    let base_url = if let Some(state) = resume {
        state.resume_url.clone()
    } else {
        api::get_gateway_url(http, bot_token).await.context("failed to get gateway URL")?
    };
    let mut parsed = url::Url::parse(&base_url).context("invalid gateway URL")?;
    parsed.query_pairs_mut().append_pair("v", "10").append_pair("encoding", "json");
    let ws_url = parsed.to_string();
    debug!(%ws_url, resuming = resume.is_some(), "connecting to Discord gateway");

    // 2. Connect with required User-Agent header
    let mut request =
        ws_url.as_str().into_client_request().context("failed to build WebSocket request")?;
    request.headers_mut().insert("User-Agent", "DiscordBot (hivemind, 1.0)".parse().unwrap());
    let (ws_stream, _response) =
        tokio_tungstenite::connect_async(request).await.context("WebSocket connect failed")?;
    let (mut sink, mut stream) = ws_stream.split();

    // 3. Receive HELLO (opcode 10)
    let hello = read_json(&mut stream).await.context("expected HELLO")?;
    let heartbeat_interval_ms =
        hello["d"]["heartbeat_interval"].as_u64().context("missing heartbeat_interval in HELLO")?;
    debug!(heartbeat_interval_ms, "received HELLO");

    // 4. Send RESUME or IDENTIFY
    if let Some(state) = resume {
        let resume_payload = serde_json::json!({
            "op": 6,
            "d": {
                "token": bot_token,
                "session_id": state.session_id,
                "seq": state.seq
            }
        });
        sink.send(WsMessage::Text(resume_payload.to_string().into()))
            .await
            .context("failed to send RESUME")?;
        debug!(session_id = %state.session_id, seq = ?state.seq, "sent RESUME");
    } else {
        // Intents: GUILDS (1) | GUILD_MESSAGES (512) | MESSAGE_CONTENT (32768) = 33281
        let identify = serde_json::json!({
            "op": 2,
            "d": {
                "token": bot_token,
                "intents": 33281,
                "properties": {
                    "os": "windows",
                    "browser": "hivemind",
                    "device": "hivemind"
                }
            }
        });
        sink.send(WsMessage::Text(identify.to_string().into()))
            .await
            .context("failed to send IDENTIFY")?;
    }

    // 5. Session state — initialize from resume state if available
    let seq: Arc<parking_lot::Mutex<Option<u64>>> =
        Arc::new(parking_lot::Mutex::new(resume.and_then(|s| s.seq)));
    let mut session_id: Option<String> = resume.map(|s| s.session_id.clone());
    let mut resume_url: Option<String> = resume.map(|s| s.resume_url.clone());

    let seq_hb = Arc::clone(&seq);

    // Shared sink for heartbeat + main loop
    let sink = Arc::new(tokio::sync::Mutex::new(sink));
    let sink_hb = Arc::clone(&sink);

    // Heartbeat ACK tracking — detect zombie connections per Discord protocol
    let ack_pending = Arc::new(AtomicBool::new(false));
    let ack_hb = Arc::clone(&ack_pending);

    // Spawn heartbeat task
    let heartbeat_handle = tokio::spawn(async move {
        let interval = Duration::from_millis(heartbeat_interval_ms);
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await; // first tick is immediate — skip it
        loop {
            ticker.tick().await;
            // If previous heartbeat was not ACKed, connection is zombie — close it
            if ack_hb.load(Ordering::SeqCst) {
                warn!("Discord gateway: missed heartbeat ACK, closing zombie connection");
                let _ = sink_hb.lock().await.close().await;
                return;
            }
            ack_hb.store(true, Ordering::SeqCst);
            let s = *seq_hb.lock();
            let payload = serde_json::json!({ "op": 1, "d": s });
            let msg = WsMessage::Text(payload.to_string().into());
            if sink_hb.lock().await.send(msg).await.is_err() {
                debug!("heartbeat: sink closed, stopping");
                return;
            }
        }
    });

    // Helper: build current resume state from captured session info
    let build_resume = |seq: &Arc<parking_lot::Mutex<Option<u64>>>,
                        session_id: &Option<String>,
                        resume_url: &Option<String>|
     -> Option<ResumeState> {
        match (session_id.as_ref(), resume_url.as_ref()) {
            (Some(sid), Some(url)) => Some(ResumeState {
                session_id: sid.clone(),
                resume_url: url.clone(),
                seq: *seq.lock(),
            }),
            _ => None,
        }
    };

    // 6. Main read loop
    let exit = loop {
        let value = match read_json(&mut stream).await {
            Ok(v) => v,
            Err(e) => {
                warn!(?e, "Discord gateway read error");
                break SessionExit::Reconnect(build_resume(&seq, &session_id, &resume_url));
            }
        };

        // Track sequence number
        if let Some(s) = value["s"].as_u64() {
            *seq.lock() = Some(s);
        }

        let op = value["op"].as_u64().unwrap_or(u64::MAX);
        match op {
            // Dispatch
            0 => {
                let event_name = value["t"].as_str().unwrap_or("");
                match event_name {
                    "READY" => {
                        let user = value["d"]["user"]["username"].as_str().unwrap_or("unknown");
                        session_id = value["d"]["session_id"].as_str().map(String::from);
                        resume_url = value["d"]["resume_gateway_url"].as_str().map(String::from);
                        info!(user, session_id = ?session_id, "Discord gateway READY");
                    }
                    "RESUMED" => {
                        info!("Discord gateway RESUMED successfully");
                    }
                    "MESSAGE_CREATE" => {
                        if let Some(msg) = parse_message_create(&value["d"]) {
                            // Filter by guild
                            if !allowed_guild_ids.is_empty() {
                                if let Some(ref gid) = msg.guild_id {
                                    if !allowed_guild_ids.contains(gid) {
                                        debug!(guild_id = %gid, "Discord gateway: skipping message from non-allowed guild");
                                        continue;
                                    }
                                }
                            }
                            // Filter by channel
                            if !listen_channel_ids.is_empty()
                                && !listen_channel_ids.contains(&msg.channel_id)
                            {
                                debug!(channel_id = %msg.channel_id, "Discord gateway: skipping message from non-listened channel");
                                continue;
                            }

                            debug!(
                                message_id = %msg.message_id,
                                channel_id = %msg.channel_id,
                                author = %msg.author_name,
                                referenced_message_id = ?msg.referenced_message_id,
                                "Discord gateway: received MESSAGE_CREATE"
                            );
                            if tx.send(msg).await.is_err() {
                                break SessionExit::ReceiverDropped;
                            }
                            notify.notify_waiters();
                        }
                    }
                    "INTERACTION_CREATE" => {
                        if let Some(msg) = parse_interaction_create(&value["d"]) {
                            debug!(
                                custom_id = %msg.interaction.as_ref().map(|i| i.custom_id.as_str()).unwrap_or(""),
                                "Discord gateway: received INTERACTION_CREATE"
                            );
                            if tx.send(msg).await.is_err() {
                                break SessionExit::ReceiverDropped;
                            }
                            notify.notify_waiters();
                        }
                    }
                    _ => {
                        debug!(event_name, "unhandled dispatch event");
                    }
                }
            }
            // Heartbeat ACK
            11 => {
                ack_pending.store(false, Ordering::SeqCst);
            }
            // Reconnect
            7 => {
                info!("Discord gateway requested reconnect (op 7)");
                break SessionExit::Reconnect(build_resume(&seq, &session_id, &resume_url));
            }
            // Invalid session
            9 => {
                let resumable = value["d"].as_bool().unwrap_or(false);
                warn!(resumable, "Discord gateway invalid session (op 9)");
                if resumable {
                    break SessionExit::Reconnect(build_resume(&seq, &session_id, &resume_url));
                } else {
                    break SessionExit::InvalidSession;
                }
            }
            // Heartbeat request
            1 => {
                let s = *seq.lock();
                let payload = serde_json::json!({ "op": 1, "d": s });
                let msg = WsMessage::Text(payload.to_string().into());
                if sink.lock().await.send(msg).await.is_err() {
                    warn!("heartbeat send failed; connection may be closed");
                }
            }
            other => {
                debug!(op = other, "unhandled gateway opcode");
            }
        }
    };

    heartbeat_handle.abort();
    exit.into()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Read the next text frame from the WebSocket and parse it as JSON.
async fn read_json<S>(stream: &mut S) -> Result<serde_json::Value>
where
    S: StreamExt<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let frame = stream
            .next()
            .await
            .context("WebSocket stream ended")?
            .context("WebSocket read error")?;

        match frame {
            WsMessage::Text(text) => {
                let value: serde_json::Value =
                    serde_json::from_str(&text).context("invalid JSON from gateway")?;
                return Ok(value);
            }
            WsMessage::Close(_) => {
                anyhow::bail!("WebSocket closed by server");
            }
            // Ignore ping/pong/binary
            _ => continue,
        }
    }
}

/// Parse a MESSAGE_CREATE `d` payload into a `GatewayMessage`.
/// Returns `None` if the author is a bot or essential fields are missing.
fn parse_message_create(d: &serde_json::Value) -> Option<GatewayMessage> {
    // Skip bot messages
    if d["author"]["bot"].as_bool().unwrap_or(false) {
        return None;
    }

    let channel_id = d["channel_id"].as_str()?.to_string();
    let guild_id = d["guild_id"].as_str().map(String::from);
    let author_id = d["author"]["id"].as_str()?.to_string();
    let author_name = d["author"]["username"].as_str()?.to_string();
    let content = d["content"].as_str().unwrap_or("").to_string();
    let message_id = d["id"].as_str()?.to_string();
    let timestamp = d["timestamp"].as_str().unwrap_or("").to_string();

    // Extract reply reference (present when user replies to a message)
    let referenced_message_id = d
        .get("message_reference")
        .and_then(|mr| mr.get("message_id"))
        .and_then(|v| v.as_str())
        .map(String::from);

    Some(GatewayMessage {
        channel_id,
        guild_id,
        author_id,
        author_name,
        content,
        message_id,
        timestamp,
        referenced_message_id,
        interaction: None,
    })
}

/// Parse an INTERACTION_CREATE `d` payload (component button clicks).
fn parse_interaction_create(d: &serde_json::Value) -> Option<GatewayMessage> {
    // Only handle component interactions (type 3)
    if d["type"].as_u64()? != 3 {
        return None;
    }

    let data = d.get("data")?;
    let custom_id = data.get("custom_id")?.as_str()?.to_string();
    let interaction_id = d["id"].as_str()?.to_string();
    let interaction_token = d["token"].as_str()?.to_string();

    let channel_id = d["channel_id"].as_str().unwrap_or("").to_string();
    let guild_id = d["guild_id"].as_str().map(String::from);

    let member = d.get("member");
    let user = member.and_then(|m| m.get("user")).or_else(|| d.get("user"));

    let author_id =
        user.and_then(|u| u.get("id")).and_then(|v| v.as_str()).unwrap_or("").to_string();
    let author_name =
        user.and_then(|u| u.get("username")).and_then(|v| v.as_str()).unwrap_or("").to_string();

    let message_id = d
        .get("message")
        .and_then(|m| m.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Some(GatewayMessage {
        channel_id,
        guild_id,
        author_id,
        author_name,
        content: String::new(),
        message_id,
        timestamp: String::new(),
        referenced_message_id: None,
        interaction: Some(DiscordInteraction { custom_id, interaction_id, interaction_token }),
    })
}

// Allow `Ok(exit)` → `exit.into()` in the session function above.
impl From<SessionExit> for Result<SessionExit> {
    fn from(v: SessionExit) -> Self {
        Ok(v)
    }
}
