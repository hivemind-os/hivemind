//! Streamable HTTP transport for MCP (Model Context Protocol).
//!
//! The Streamable HTTP transport sends each JSON-RPC message as a POST request
//! and reads the response which may be either a single JSON object or an SSE
//! stream.  A background GET SSE connection is maintained to receive
//! server-initiated messages (notifications and requests).
//!
//! See <https://spec.modelcontextprotocol.io/specification/basic/transports/#streamable-http>.

use futures::{FutureExt, Sink, Stream};
use reqwest::{Client as HttpClient, Url};
use rmcp::model::{ClientJsonRpcMessage, ServerJsonRpcMessage};
use std::{
    collections::VecDeque,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

#[derive(Error, Debug)]
pub enum StreamableHttpError {
    #[error("Reqwest error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("URL parse error: {0}")]
    Url(String),
    #[error("channel closed")]
    ChannelClosed,
    #[error("Malformed response: {0}")]
    MalformedResponse(String),
}

/// A Streamable HTTP MCP client transport.
///
/// Each outgoing JSON-RPC message is POSTed to the endpoint URL.  The server
/// responds with either `application/json` (single response) or
/// `text/event-stream` (SSE with one or more responses).  The server may
/// return a `Mcp-Session-Id` header which must be echoed back on all
/// subsequent requests.
pub struct StreamableHttpTransport {
    url: Url,
    client: HttpClient,
    session_id_tx: Arc<watch::Sender<Option<String>>>,
    /// Receives server responses produced by background POST tasks.
    response_rx: mpsc::Receiver<ServerJsonRpcMessage>,
    /// Sender cloned into each POST task.
    response_tx: mpsc::Sender<ServerJsonRpcMessage>,
    /// Outstanding POST futures we need to poll for back-pressure.
    pending_sends: VecDeque<tokio::sync::oneshot::Receiver<Result<(), StreamableHttpError>>>,
    cancel_token: CancellationToken,
    listener_handle: Option<tokio::task::JoinHandle<()>>,
}

impl StreamableHttpTransport {
    /// Create a new transport pointing at the given MCP endpoint URL.
    pub fn new<U: reqwest::IntoUrl>(url: U) -> Result<Self, StreamableHttpError> {
        let client = HttpClient::builder().timeout(std::time::Duration::from_secs(60)).build()?;
        Self::new_with_client(url, client)
    }

    /// Create a new transport with a pre-configured HTTP client (e.g. with
    /// default headers for authentication).
    pub fn new_with_client<U: reqwest::IntoUrl>(
        url: U,
        client: HttpClient,
    ) -> Result<Self, StreamableHttpError> {
        let url = url.into_url().map_err(|e| StreamableHttpError::Url(e.to_string()))?;
        let (response_tx, response_rx) = mpsc::channel(256);
        let (session_id_tx, session_id_rx) = watch::channel(None);
        let session_id_tx = Arc::new(session_id_tx);
        let cancel_token = CancellationToken::new();

        let listener_handle = tokio::spawn(listen_for_server_messages(
            url.clone(),
            client.clone(),
            session_id_rx,
            response_tx.clone(),
            cancel_token.clone(),
        ));

        Ok(Self {
            url,
            client,
            session_id_tx,
            response_rx,
            response_tx,
            pending_sends: VecDeque::new(),
            cancel_token,
            listener_handle: Some(listener_handle),
        })
    }

    /// Cancel the background listener and abort its task.
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
        if let Some(handle) = &self.listener_handle {
            handle.abort();
        }
    }
}

impl Drop for StreamableHttpTransport {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        if let Some(handle) = self.listener_handle.take() {
            handle.abort();
        }
    }
}

impl Stream for StreamableHttpTransport {
    type Item = ServerJsonRpcMessage;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.response_rx.poll_recv(cx)
    }
}

impl Sink<ClientJsonRpcMessage> for StreamableHttpTransport {
    type Error = StreamableHttpError;

    fn poll_ready(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        const QUEUE_SIZE: usize = 16;
        if self.pending_sends.len() >= QUEUE_SIZE {
            if let Some(front) = self.pending_sends.front_mut() {
                let result = std::task::ready!(front.poll_unpin(cx));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => return Poll::Ready(Err(e)),
                    Err(_) => return Poll::Ready(Err(StreamableHttpError::ChannelClosed)),
                }
                self.pending_sends.pop_front();
            }
        }
        Poll::Ready(Ok(()))
    }

    fn start_send(mut self: Pin<&mut Self>, item: ClientJsonRpcMessage) -> Result<(), Self::Error> {
        let client = self.client.clone();
        let url = self.url.clone();
        let tx = self.response_tx.clone();
        let session_id_tx = Arc::clone(&self.session_id_tx);
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();

        tokio::spawn(async move {
            let result = post_and_parse(client, url, item, tx, session_id_tx).await;
            let _ = done_tx.send(result);
        });

        self.pending_sends.push_back(done_rx);
        Ok(())
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        while let Some(fut) = self.pending_sends.front_mut() {
            let result = std::task::ready!(fut.poll_unpin(cx));
            self.pending_sends.pop_front();
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => return Poll::Ready(Err(e)),
                Err(_) => return Poll::Ready(Err(StreamableHttpError::ChannelClosed)),
            }
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.poll_flush(cx)
    }
}

/// Truncate a string to at most `max_len` bytes on a valid UTF-8 boundary.
fn truncate(s: &str, max_len: usize) -> &str {
    if s.len() <= max_len {
        s
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

/// Compute a backoff delay with jitter.
/// The delay is `base_secs + [0, base_secs/2)` using the system clock as a
/// lightweight source of jitter (no `rand` dependency required).
fn backoff_delay(base_secs: u64) -> std::time::Duration {
    let half_ms = base_secs * 500;
    let jitter_ms = if half_ms > 0 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| (d.subsec_nanos() as u64) % half_ms)
            .unwrap_or(0)
    } else {
        0
    };
    std::time::Duration::from_millis(base_secs * 1000 + jitter_ms)
}

/// POST a single JSON-RPC message and parse the response.
///
/// The server may respond with:
///  - `application/json`: a single JSON-RPC response (or a JSON array of them)
///  - `text/event-stream`: SSE stream of `message` events containing JSON-RPC
///
/// If the server returns an `Mcp-Session-Id` header, it is stored via the
/// shared watch channel and sent on all subsequent requests.
async fn post_and_parse(
    client: HttpClient,
    url: Url,
    message: ClientJsonRpcMessage,
    tx: mpsc::Sender<ServerJsonRpcMessage>,
    session_id_tx: Arc<watch::Sender<Option<String>>>,
) -> Result<(), StreamableHttpError> {
    let mut request = client
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream");

    // Attach session ID if we have one from a previous response.
    {
        let current = session_id_tx.borrow().clone();
        if let Some(ref sid) = current {
            request = request.header("Mcp-Session-Id", sid.as_str());
        }
    }

    let response = request.json(&message).send().await?.error_for_status()?;

    let status = response.status();

    // 2xx with no content expected: server acknowledged a notification/response.
    // MCP Streamable HTTP returns 202 for notifications, but other codes are possible.
    if status == reqwest::StatusCode::ACCEPTED || status == reqwest::StatusCode::NO_CONTENT {
        // Still capture session ID if present.
        if let Some(sid) = response.headers().get("Mcp-Session-Id").and_then(|v| v.to_str().ok()) {
            let current = session_id_tx.borrow().clone();
            if current.is_none() {
                let _ = session_id_tx.send(Some(sid.to_string()));
            }
        }
        return Ok(());
    }

    // Capture session ID from the response if the server provides one.
    if let Some(sid) = response.headers().get("Mcp-Session-Id").and_then(|v| v.to_str().ok()) {
        let current = session_id_tx.borrow().clone();
        if let Some(ref existing) = current {
            if existing != sid {
                tracing::warn!(
                    existing = %existing,
                    received = sid,
                    "streamable-http: ignoring conflicting session id"
                );
            }
        } else {
            let _ = session_id_tx.send(Some(sid.to_string()));
        }
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    if content_type.starts_with("text/event-stream") {
        // SSE response — read events and extract JSON-RPC messages.
        use futures::StreamExt;
        use sse_stream::SseStream;

        let byte_stream = response.bytes_stream();
        let mut sse = SseStream::from_byte_stream(byte_stream).boxed();

        while let Some(event_result) = sse.next().await {
            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "streamable-http: SSE stream error in POST response");
                    break;
                }
            };
            // Only process "message" events (the MCP default event type).
            if event.event.as_deref().unwrap_or("message") != "message" {
                continue;
            }
            if let Some(data) = &event.data {
                if let Ok(msg) = serde_json::from_str::<ServerJsonRpcMessage>(data) {
                    if tx.send(msg).await.is_err() {
                        tracing::warn!("streamable-http: response channel closed");
                        return Ok(());
                    }
                } else if let Ok(batch) = serde_json::from_str::<Vec<ServerJsonRpcMessage>>(data) {
                    for msg in batch {
                        if tx.send(msg).await.is_err() {
                            tracing::warn!("streamable-http: response channel closed");
                            return Ok(());
                        }
                    }
                } else {
                    tracing::warn!(
                        data = %truncate(data, 200),
                        "streamable-http: failed to parse SSE data as JSON-RPC in POST response"
                    );
                }
            }
        }
    } else {
        // JSON response — may be a single message or an array.
        let body = response.text().await?;
        if body.is_empty() {
            // Empty body with 2xx — server has nothing to return for this message.
            tracing::debug!(
                status = %status,
                content_type = %content_type,
                "streamable-http: empty response body, treating as accepted"
            );
            return Ok(());
        }
        if let Ok(msg) = serde_json::from_str::<ServerJsonRpcMessage>(&body) {
            if tx.send(msg).await.is_err() {
                tracing::warn!("streamable-http: response channel closed");
                return Ok(());
            }
        } else if let Ok(batch) = serde_json::from_str::<Vec<ServerJsonRpcMessage>>(&body) {
            for msg in batch {
                if tx.send(msg).await.is_err() {
                    tracing::warn!("streamable-http: response channel closed");
                    return Ok(());
                }
            }
        } else {
            tracing::warn!(
                status = %status,
                content_type = %content_type,
                body = %truncate(&body, 200),
                "streamable-http: malformed JSON response"
            );
            return Err(StreamableHttpError::MalformedResponse(truncate(&body, 200).to_string()));
        }
    }

    Ok(())
}

/// Background task that opens a GET SSE connection to receive server-initiated
/// messages (notifications, requests) per the MCP Streamable HTTP spec.
///
/// The task waits for the session ID to be set (via the watch channel during
/// the initialize POST handshake), then opens a persistent GET request with
/// `Accept: text/event-stream`.  On disconnect it automatically reconnects
/// with exponential backoff, passing `Last-Event-ID` for resumption when the
/// server supports it.  If the server returns 405 Method Not Allowed, the task
/// exits quietly since the server does not support server-initiated messages.
/// HTTP 401/403 are treated as permanent failures.
async fn listen_for_server_messages(
    url: Url,
    base_client: HttpClient,
    mut session_id_rx: watch::Receiver<Option<String>>,
    tx: mpsc::Sender<ServerJsonRpcMessage>,
    cancel_token: CancellationToken,
) {
    // Clone default headers from the base client, but remove the overall
    // request timeout — the SSE stream is expected to remain open indefinitely.
    let _ = base_client; // used below only for its default headers
    let client =
        match HttpClient::builder().connect_timeout(std::time::Duration::from_secs(120)).build() {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "streamable-http: failed to build GET listener client");
                return;
            }
        };

    // Wait for the session ID via the watch channel.
    let sid = loop {
        tokio::select! {
            _ = cancel_token.cancelled() => return,
            result = session_id_rx.changed() => {
                match result {
                    Ok(()) => {
                        let value = session_id_rx.borrow_and_update().clone();
                        if let Some(sid) = value {
                            break sid;
                        }
                    }
                    Err(_) => return, // sender dropped
                }
            }
        }
    };

    tracing::debug!("streamable-http: starting GET SSE listener");

    let mut last_event_id: Option<String> = None;
    let mut consecutive_failures: u32 = 0;
    let mut backoff_secs: u64 = 1;
    const MAX_CONSECUTIVE_FAILURES: u32 = 20;
    const MAX_BACKOFF_SECS: u64 = 60;

    loop {
        if cancel_token.is_cancelled() || tx.is_closed() {
            return;
        }

        let mut request = client
            .get(url.clone())
            .header("Accept", "text/event-stream")
            .header("Mcp-Session-Id", &sid);

        if let Some(ref id) = last_event_id {
            request = request.header("Last-Event-ID", id);
        }

        let response = tokio::select! {
            _ = cancel_token.cancelled() => return,
            result = request.send() => result,
        };

        let response = match response {
            Ok(r) if r.status().is_success() => {
                consecutive_failures = 0;
                backoff_secs = 1;
                r
            }
            Ok(r) if r.status() == reqwest::StatusCode::METHOD_NOT_ALLOWED => {
                tracing::debug!(
                    "streamable-http: server returned 405 for GET — \
                     server-initiated messages not supported"
                );
                return;
            }
            Ok(r)
                if r.status() == reqwest::StatusCode::UNAUTHORIZED
                    || r.status() == reqwest::StatusCode::FORBIDDEN =>
            {
                tracing::error!(
                    status = %r.status(),
                    "streamable-http: GET SSE permanently rejected (auth failure)"
                );
                return;
            }
            Ok(r) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::error!(
                        failures = consecutive_failures,
                        "streamable-http: GET SSE listener exhausted max retries, exiting"
                    );
                    return;
                }
                tracing::warn!(
                    status = %r.status(),
                    attempt = consecutive_failures,
                    "streamable-http: GET SSE request rejected, retrying with backoff"
                );
                let delay = backoff_delay(backoff_secs);
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                tokio::select! {
                    _ = cancel_token.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {},
                }
                continue;
            }
            Err(e) => {
                consecutive_failures += 1;
                if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                    tracing::error!(
                        failures = consecutive_failures,
                        "streamable-http: GET SSE listener exhausted max retries, exiting"
                    );
                    return;
                }
                tracing::warn!(
                    error = %e,
                    attempt = consecutive_failures,
                    "streamable-http: GET SSE connection failed, retrying with backoff"
                );
                let delay = backoff_delay(backoff_secs);
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
                tokio::select! {
                    _ = cancel_token.cancelled() => return,
                    _ = tokio::time::sleep(delay) => {},
                }
                continue;
            }
        };

        // Read SSE events from the GET stream.
        use futures::StreamExt;
        use sse_stream::SseStream;

        let byte_stream = response.bytes_stream();
        let mut sse = SseStream::from_byte_stream(byte_stream).boxed();

        loop {
            let event_result = tokio::select! {
                _ = cancel_token.cancelled() => return,
                item = sse.next() => {
                    match item {
                        Some(r) => r,
                        None => break,
                    }
                }
            };

            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!(error = %e, "streamable-http: GET SSE stream error");
                    break;
                }
            };

            // Track last event ID for resumption on reconnect.
            if let Some(ref id) = event.id {
                last_event_id = Some(id.clone());
            }

            if event.event.as_deref().unwrap_or("message") != "message" {
                continue;
            }

            if let Some(data) = &event.data {
                if let Ok(msg) = serde_json::from_str::<ServerJsonRpcMessage>(data) {
                    tokio::select! {
                        _ = cancel_token.cancelled() => return,
                        result = tx.send(msg) => {
                            if result.is_err() {
                                return;
                            }
                        }
                    }
                } else if let Ok(batch) = serde_json::from_str::<Vec<ServerJsonRpcMessage>>(data) {
                    for msg in batch {
                        tokio::select! {
                            _ = cancel_token.cancelled() => return,
                            result = tx.send(msg) => {
                                if result.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                } else {
                    tracing::warn!(
                        data = %truncate(data, 200),
                        "streamable-http: GET SSE failed to parse message"
                    );
                }
            }
        }

        // Stream disconnected — reconnect after a short delay.
        tracing::debug!("streamable-http: GET SSE stream disconnected, reconnecting");
        tokio::select! {
            _ = cancel_token.cancelled() => return,
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {},
        }
    }
}
