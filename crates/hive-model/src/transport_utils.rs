//! Shared utilities used by all `ProviderTransport` implementations.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::RequestBuilder;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, HashMap};

use crate::{get_copilot_token_blocking, read_env, read_keyring, ProviderAuth, ProviderKind};
use crate::{
    parse_sse_data, restore_tool_name_with_map, CompletionChunk, CompletionStream, FinishReason,
    ToolCallArgDelta, ToolCallDelta, ToolCallResponse,
};

use tokio_stream::StreamExt as _;

// ---------------------------------------------------------------------------
// Blocking helpers
// ---------------------------------------------------------------------------

/// Apply authentication and extra headers to a blocking request builder.
pub(crate) fn apply_blocking_auth(
    mut request: RequestBuilder,
    auth: &ProviderAuth,
    extra_headers: &BTreeMap<String, String>,
    _provider_id: &str,
) -> Result<RequestBuilder> {
    for (name, value) in extra_headers {
        request = request.header(name.as_str(), value.as_str());
    }

    match auth {
        ProviderAuth::None => Ok(request),
        ProviderAuth::BearerEnv(env_var) => Ok(request.bearer_auth(read_env(env_var)?)),
        ProviderAuth::HeaderEnv { env_var, header_name } => {
            Ok(request.header(header_name.as_str(), read_env(env_var)?))
        }
        ProviderAuth::BearerKeyring { key } => {
            let secret = read_keyring(key)?;
            Ok(request.bearer_auth(secret))
        }
        ProviderAuth::HeaderKeyring { key, header_name } => {
            let secret = read_keyring(key)?;
            Ok(request.header(header_name.as_str(), secret))
        }
        ProviderAuth::GitHubToken => {
            let token = read_keyring("github:oauth-token").or_else(|_| {
                std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")).context(
                    "github oauth provider auth requires a saved GitHub token or GITHUB_TOKEN/GH_TOKEN env var",
                )
            })?;
            Ok(request.bearer_auth(token))
        }
        ProviderAuth::GitHubCopilotToken => {
            let token = get_copilot_token_blocking()?;
            Ok(request
                .bearer_auth(token)
                .header("Editor-Version", "HiveMind OS/0.1.0")
                .header("Editor-Plugin-Version", "copilot/0.1.0")
                .header("Copilot-Integration-Id", "vscode-chat"))
        }
    }
}

/// Send an authenticated, blocking JSON request and deserialise the response.
pub(crate) fn send_json_blocking<T: DeserializeOwned>(
    request: RequestBuilder,
    auth: &ProviderAuth,
    extra_headers: &BTreeMap<String, String>,
    provider_id: &str,
) -> Result<T> {
    let request = apply_blocking_auth(request, auth, extra_headers, provider_id)?;
    let response =
        request.send().with_context(|| format!("failed to contact provider {}", provider_id))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_else(|_| "<unavailable>".to_string());
        bail!("provider {} returned {}: {}", provider_id, status, body);
    }

    response
        .json::<T>()
        .with_context(|| format!("provider {} returned malformed json", provider_id))
}

// ---------------------------------------------------------------------------
// Async helpers
// ---------------------------------------------------------------------------

/// Apply authentication and extra headers to an async request builder.
pub(crate) fn apply_async_auth(
    mut request: reqwest::RequestBuilder,
    auth: &ProviderAuth,
    extra_headers: &BTreeMap<String, String>,
    _provider_id: &str,
) -> Result<reqwest::RequestBuilder> {
    for (name, value) in extra_headers {
        request = request.header(name.as_str(), value.as_str());
    }

    match auth {
        ProviderAuth::None => Ok(request),
        ProviderAuth::BearerEnv(env_var) => Ok(request.bearer_auth(read_env(env_var)?)),
        ProviderAuth::HeaderEnv { env_var, header_name } => {
            Ok(request.header(header_name.as_str(), read_env(env_var)?))
        }
        ProviderAuth::BearerKeyring { key } => {
            let secret = read_keyring(key)?;
            Ok(request.bearer_auth(secret))
        }
        ProviderAuth::HeaderKeyring { key, header_name } => {
            let secret = read_keyring(key)?;
            Ok(request.header(header_name.as_str(), secret))
        }
        ProviderAuth::GitHubToken => {
            let token = read_keyring("github:oauth-token").or_else(|_| {
                std::env::var("GITHUB_TOKEN").or_else(|_| std::env::var("GH_TOKEN")).context(
                    "github oauth provider auth requires a saved GitHub token or GITHUB_TOKEN/GH_TOKEN env var",
                )
            })?;
            Ok(request.bearer_auth(token))
        }
        ProviderAuth::GitHubCopilotToken => {
            let token =
                get_copilot_token_blocking().context("failed to get Copilot session token")?;
            Ok(request
                .bearer_auth(token)
                .header("Editor-Version", "HiveMind OS/0.1.0")
                .header("Editor-Plugin-Version", "copilot/0.1.0")
                .header("Copilot-Integration-Id", "vscode-chat"))
        }
    }
}

// ---------------------------------------------------------------------------
// SSE streaming – shared by all transports
// ---------------------------------------------------------------------------

/// Build a `CompletionStream` from an already-authenticated SSE request.
///
/// This contains the SSE parsing loop, tool-call accumulation, and chunk
/// assembly that is common to every provider protocol.
pub(crate) fn sse_completion_stream(
    url: String,
    req_builder: reqwest::RequestBuilder,
    kind: ProviderKind,
    provider_id: String,
    tool_name_map: HashMap<String, String>,
) -> CompletionStream {
    let stream = async_stream::try_stream! {
        let response = req_builder
            .send()
            .await
            .with_context(|| format!("failed to contact provider {provider_id} at {url}"))?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_else(|_| "<unavailable>".to_string());
            Err(anyhow!("provider {provider_id} returned {status}: {body}"))?;
            return; // unreachable, but helps the compiler
        }

        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();
        const MAX_SSE_BUFFER_SIZE: usize = 10 * 1024 * 1024; // 10 MB

        // Accumulation state for tool calls streamed across multiple SSE chunks.
        // Each entry: (id, name, arguments_buffer)
        let mut pending_tool_calls: Vec<(String, String, String)> = vec![];
        const MAX_TOOL_CALL_INDEX: usize = 1000;

        while let Some(chunk_result) = byte_stream.next().await {
            let bytes = chunk_result
                .with_context(|| format!("provider {provider_id} stream read error"))?;
            buffer.push_str(&String::from_utf8_lossy(&bytes));

            if buffer.len() > MAX_SSE_BUFFER_SIZE {
                Err(anyhow!("provider {} SSE buffer exceeded {}MB limit", provider_id, MAX_SSE_BUFFER_SIZE / (1024 * 1024)))?;
                return;
            }

            // Process complete lines from the buffer
            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() || line.starts_with(':') {
                    continue;
                }

                if line.starts_with("event: ") {
                    // Anthropic uses event lines; we handle data lines below
                    continue;
                }

                if let Some(data) = line.strip_prefix("data: ") {
                    if data.trim() == "[DONE]" {
                        return;
                    }

                    let result = parse_sse_data(data, &kind, &provider_id)?;

                    // Accumulate tool call deltas and build snapshot entries
                    let mut arg_deltas = Vec::new();
                    for delta in result.tool_call_deltas {
                        match delta {
                            ToolCallDelta::OpenAi { index, id, name, arguments } => {
                                if index > MAX_TOOL_CALL_INDEX {
                                    Err(anyhow!(
                                        "provider {provider_id} tool call index {index} exceeds limit {MAX_TOOL_CALL_INDEX}"
                                    ))?;
                                    return;
                                }
                                // Grow the pending list if needed
                                while pending_tool_calls.len() <= index {
                                    pending_tool_calls.push((String::new(), String::new(), String::new()));
                                }
                                if let Some(id) = id {
                                    pending_tool_calls[index].0 = id;
                                }
                                if let Some(name) = name {
                                    pending_tool_calls[index].1 = name;
                                }
                                if let Some(args) = arguments {
                                    pending_tool_calls[index].2.push_str(&args);
                                }
                                // Emit snapshot of accumulated state
                                let entry = &pending_tool_calls[index];
                                arg_deltas.push(ToolCallArgDelta {
                                    index,
                                    call_id: if entry.0.is_empty() { None } else { Some(entry.0.clone()) },
                                    name: if entry.1.is_empty() { None } else { Some(entry.1.clone()) },
                                    arguments_so_far: entry.2.clone(),
                                });
                            }
                            ToolCallDelta::AnthropicStart { id, name } => {
                                pending_tool_calls.push((id.clone(), name.clone(), String::new()));
                                let index = pending_tool_calls.len() - 1;
                                arg_deltas.push(ToolCallArgDelta {
                                    index,
                                    call_id: Some(id),
                                    name: Some(name),
                                    arguments_so_far: String::new(),
                                });
                            }
                            ToolCallDelta::AnthropicArgsDelta { partial_json } => {
                                let index = pending_tool_calls.len().saturating_sub(1);
                                if let Some(last) = pending_tool_calls.last_mut() {
                                    last.2.push_str(&partial_json);
                                    arg_deltas.push(ToolCallArgDelta {
                                        index,
                                        call_id: if last.0.is_empty() { None } else { Some(last.0.clone()) },
                                        name: if last.1.is_empty() { None } else { Some(last.1.clone()) },
                                        arguments_so_far: last.2.clone(),
                                    });
                                }
                            }
                            ToolCallDelta::AnthropicStop => {
                                // Nothing special needed; entry is already accumulated.
                            }
                        }
                    }

                    if let Some(mut chunk) = result.chunk {
                        // Attach partial arg snapshots to the chunk
                        chunk.tool_call_arg_deltas = arg_deltas;
                        // Attach accumulated tool calls on the final tool-calls chunk.
                        if chunk.finish_reason == Some(FinishReason::ToolCalls) && !pending_tool_calls.is_empty() {
                            chunk.tool_calls = pending_tool_calls
                                .drain(..)
                                .filter(|(_, name, _)| !name.is_empty())
                                .map(|(id, name, args)| {
                                    let arguments = serde_json::from_str(&args)
                                        .unwrap_or(serde_json::Value::Null);
                                    ToolCallResponse { id, name: restore_tool_name_with_map(&name, &tool_name_map), arguments }
                                })
                                .collect();
                        }
                        yield chunk;
                    } else if !arg_deltas.is_empty() {
                        // No text/finish chunk but we have arg deltas — emit
                        // a delta-only chunk so they reach downstream.
                        yield CompletionChunk {
                            delta: String::new(),
                            finish_reason: None,
                            tool_calls: vec![],
                            tool_call_arg_deltas: arg_deltas,
                        };
                    }
                }
            }
        }
    };

    Box::pin(stream)
}
