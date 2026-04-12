//! Anthropic transport implementation.

use anyhow::{bail, Result};

use crate::transport::ProviderTransport;
use crate::transport::TransportContext;
use crate::transport_utils::{apply_async_auth, send_json_blocking, sse_completion_stream};
use crate::{
    anthropic_messages_from_request, build_tool_name_map, format_tools_anthropic,
    restore_tool_name_with_map, shared_async_client, shared_blocking_client, trim_trailing_slash,
    AnthropicRequest, AnthropicResponse, AnthropicStreamRequest, CompletionRequest,
    CompletionResponse, CompletionStream, ModelSelection, ToolCallResponse,
};

pub(crate) struct AnthropicTransport;

impl ProviderTransport for AnthropicTransport {
    fn complete_blocking(
        &self,
        ctx: &TransportContext<'_>,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        let url = format!("{}/v1/messages", trim_trailing_slash(ctx.base_url));
        let payload = AnthropicRequest {
            model: selection.model.clone(),
            max_tokens: 1024,
            messages: anthropic_messages_from_request(request),
            tools: format_tools_anthropic(&request.tools),
        };

        let tool_name_map = build_tool_name_map(&request.tools);

        let client = shared_blocking_client();
        let response: AnthropicResponse = send_json_blocking(
            client.post(url).header("anthropic-version", "2023-06-01").json(&payload),
            ctx.auth,
            ctx.extra_headers,
            ctx.provider_id,
        )?;

        let tool_calls = response
            .content
            .iter()
            .filter_map(|block| {
                if block.block_type.as_deref() == Some("tool_use") {
                    let sanitized_name = block.name.clone()?;
                    Some(ToolCallResponse {
                        id: block.id.clone().unwrap_or_default(),
                        name: restore_tool_name_with_map(&sanitized_name, &tool_name_map),
                        arguments: block.input.clone().unwrap_or(serde_json::Value::Null),
                    })
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        let content = response
            .content
            .iter()
            .filter_map(
                |b| if b.block_type.as_deref() == Some("text") { b.text.clone() } else { None },
            )
            .collect::<Vec<_>>()
            .join("");

        if content.trim().is_empty() && tool_calls.is_empty() {
            bail!(
                "provider {} returned no text blocks in the anthropic response body",
                ctx.provider_id
            );
        }

        Ok(CompletionResponse {
            provider_id: ctx.provider_id.to_string(),
            model: selection.model.clone(),
            content,
            tool_calls,
        })
    }

    fn complete_stream(
        &self,
        ctx: &TransportContext<'_>,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream> {
        let client = shared_async_client().clone();
        let url = format!("{}/v1/messages", trim_trailing_slash(ctx.base_url));
        let payload = AnthropicStreamRequest {
            model: selection.model.clone(),
            max_tokens: 1024,
            messages: anthropic_messages_from_request(request),
            stream: true,
            tools: format_tools_anthropic(&request.tools),
        };
        let rb = client.post(&url).header("anthropic-version", "2023-06-01").json(&payload);
        let rb = apply_async_auth(rb, ctx.auth, ctx.extra_headers, ctx.provider_id)?;
        let provider_id = ctx.provider_id.to_string();
        let kind = ctx.provider_kind.clone();
        let tool_name_map = build_tool_name_map(&request.tools);

        Ok(sse_completion_stream(url, rb, kind, provider_id, tool_name_map))
    }
}
