use anyhow::Result;
use std::collections::BTreeMap;

use crate::{
    CompletionRequest, CompletionResponse, CompletionStream, ModelSelection, ProviderAuth,
    ProviderKind,
};

/// Bundles the provider-level context that every transport method needs.
pub(crate) struct TransportContext<'a> {
    pub base_url: &'a str,
    pub provider_id: &'a str,
    pub provider_kind: &'a ProviderKind,
    pub auth: &'a ProviderAuth,
    pub extra_headers: &'a BTreeMap<String, String>,
}

/// Protocol-specific transport for communicating with LLM providers.
///
/// Each implementation handles request shaping and response parsing for a
/// single wire protocol (OpenAI-compatible, Anthropic, Microsoft Foundry).
/// Transport structs are stateless; all necessary context is passed via
/// [`TransportContext`].
pub(crate) trait ProviderTransport: Send + Sync {
    /// Execute a blocking (synchronous) completion request.
    fn complete_blocking(
        &self,
        ctx: &TransportContext<'_>,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse>;

    /// Execute a streaming completion request.
    fn complete_stream(
        &self,
        ctx: &TransportContext<'_>,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream>;
}
