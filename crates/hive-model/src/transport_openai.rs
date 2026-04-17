//! OpenAI-compatible transport (covers OpenAI, GitHub Copilot, Ollama).

use anyhow::{anyhow, bail, Result};

use crate::transport::ProviderTransport;
use crate::transport::TransportContext;
use crate::transport_utils::{apply_async_auth, send_json_blocking, sse_completion_stream};
use crate::{
    build_tool_name_map, format_tools_openai, openai_messages_from_request,
    restore_tool_name_with_map, shared_async_client, shared_blocking_client, trim_trailing_slash,
    CompletionRequest, CompletionResponse, CompletionStream, ModelSelection, OpenAiChatRequest,
    OpenAiChatResponse, OpenAiChatStreamRequest, ToolCallResponse,
};

pub(crate) struct OpenAiTransport;

impl ProviderTransport for OpenAiTransport {
    fn complete_blocking(
        &self,
        ctx: &TransportContext<'_>,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        let url = format!("{}/chat/completions", trim_trailing_slash(ctx.base_url));
        let payload = OpenAiChatRequest {
            model: selection.model.clone(),
            messages: openai_messages_from_request(request),
            tools: format_tools_openai(&request.tools),
        };

        let tool_name_map = build_tool_name_map(&request.tools);
        let client = shared_blocking_client();
        let response: OpenAiChatResponse = send_json_blocking(
            client.post(url).json(&payload),
            ctx.auth,
            ctx.extra_headers,
            ctx.provider_id,
        )?;

        let first_choice = response.choices.into_iter().next().ok_or_else(|| {
            anyhow!("provider {} returned no choices in the response body", ctx.provider_id)
        })?;

        let msg = first_choice.message;
        let text = first_choice.text;

        let tool_calls = msg
            .as_ref()
            .and_then(|m| m.tool_calls.as_ref())
            .map(|tcs| {
                tcs.iter()
                    .filter_map(|tc| {
                        let func = tc.function.as_ref()?;
                        let name = func.name.as_ref()?.clone();
                        let args_str = func.arguments.as_ref()?;
                        let arguments =
                            serde_json::from_str(args_str).unwrap_or(serde_json::Value::Null);
                        Some(ToolCallResponse {
                            id: tc.id.clone().unwrap_or_default(),
                            name: restore_tool_name_with_map(&name, &tool_name_map),
                            arguments,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let content = msg
            .and_then(|m| m.content)
            .or(text)
            .filter(|c| !c.trim().is_empty())
            .unwrap_or_default();

        if content.is_empty() && tool_calls.is_empty() {
            bail!(
                "provider {} returned no assistant content in the response body",
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
        let url = format!("{}/chat/completions", trim_trailing_slash(ctx.base_url));
        let payload = OpenAiChatStreamRequest {
            model: selection.model.clone(),
            messages: openai_messages_from_request(request),
            stream: true,
            tools: format_tools_openai(&request.tools),
        };

        tracing::debug!(
            provider_id = %ctx.provider_id,
            provider_kind = ?ctx.provider_kind,
            model = %selection.model,
            url = %url,
            "copilot/openai request payload: {}",
            serde_json::to_string_pretty(&payload).unwrap_or_default()
        );

        let rb = client.post(&url).json(&payload);
        let rb = apply_async_auth(rb, ctx.auth, ctx.extra_headers, ctx.provider_id)?;
        let provider_id = ctx.provider_id.to_string();
        let kind = ctx.provider_kind.clone();
        let tool_name_map = build_tool_name_map(&request.tools);

        Ok(sse_completion_stream(url, rb, kind, provider_id, tool_name_map))
    }
}
