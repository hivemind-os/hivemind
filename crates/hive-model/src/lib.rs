pub mod local_provider;
pub(crate) mod transport;
pub(crate) mod transport_anthropic;
pub(crate) mod transport_foundry;
pub(crate) mod transport_openai;
pub(crate) mod transport_utils;

use anyhow::{anyhow, bail, Context, Result};
use futures_core::Stream;
use hive_contracts::ToolDefinition;
pub use hive_contracts::{Capability, ModelRouterSnapshot, ProviderDescriptor, ProviderKind};
pub use local_provider::LocalModelProvider;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::env;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use thiserror::Error;
use transport::{ProviderTransport, TransportContext};

use std::sync::atomic::{AtomicU64, Ordering};

/// Default timeout for blocking (sync) LLM calls, in seconds.
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 900;
/// Default timeout for async (streaming) LLM calls, in seconds.
pub const DEFAULT_STREAM_TIMEOUT_SECS: u64 = 900;

static REQUEST_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(DEFAULT_REQUEST_TIMEOUT_SECS);
static STREAM_TIMEOUT_SECS: AtomicU64 = AtomicU64::new(DEFAULT_STREAM_TIMEOUT_SECS);

// ── Retry constants ────────────────────────────────────────────────

/// Maximum number of same-provider retries for transient LLM errors.
const RETRY_MAX_ATTEMPTS: u32 = 5;
/// Initial backoff duration in milliseconds.
const RETRY_INITIAL_BACKOFF_MS: u64 = 5_000;
/// Multiplier applied to backoff after each retry.
const RETRY_BACKOFF_MULTIPLIER: f64 = 2.0;
/// Maximum backoff duration in milliseconds.
const RETRY_MAX_BACKOFF_MS: u64 = 40_000;

/// Retry policy configuration (internal).
#[derive(Debug, Clone)]
struct RetryPolicy {
    max_attempts: u32,
    initial_backoff_ms: u64,
    multiplier: f64,
    max_backoff_ms: u64,
}

impl RetryPolicy {
    /// Production defaults.
    fn default_policy() -> Self {
        Self {
            max_attempts: RETRY_MAX_ATTEMPTS,
            initial_backoff_ms: RETRY_INITIAL_BACKOFF_MS,
            multiplier: RETRY_BACKOFF_MULTIPLIER,
            max_backoff_ms: RETRY_MAX_BACKOFF_MS,
        }
    }

    /// Compute backoff for the given 0-based attempt, with jitter.
    fn backoff_ms(&self, attempt: u32) -> u64 {
        let base = self.initial_backoff_ms as f64 * self.multiplier.powi(attempt as i32);
        let capped = base.min(self.max_backoff_ms as f64);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let jitter_factor = 0.75 + (nanos as f64 / u32::MAX as f64) * 0.5;
        (capped * jitter_factor) as u64
    }
}

/// Compute backoff duration in ms for the given 0-based attempt, with jitter.
#[allow(dead_code)]
fn retry_backoff_ms(attempt: u32) -> u64 {
    RetryPolicy::default_policy().backoff_ms(attempt)
}

/// Information about a retry attempt, passed to the optional callback.
#[derive(Debug, Clone)]
pub struct RetryInfo {
    pub provider_id: String,
    pub model: String,
    pub attempt: u32,
    pub max_attempts: u32,
    pub error_kind: ProviderErrorKind,
    pub http_status: Option<u16>,
    pub backoff_ms: u64,
}

/// Configure LLM HTTP client timeouts. Must be called before the first LLM
/// request; later calls are ignored because the underlying clients are
/// lazily-initialised `OnceLock` singletons.
pub fn configure_timeouts(request_timeout_secs: Option<u64>, stream_timeout_secs: Option<u64>) {
    if let Some(v) = request_timeout_secs {
        REQUEST_TIMEOUT_SECS.store(v, Ordering::Relaxed);
    }
    if let Some(v) = stream_timeout_secs {
        STREAM_TIMEOUT_SECS.store(v, Ordering::Relaxed);
    }
}

/// Shared blocking HTTP client to avoid per-request allocation.
fn shared_blocking_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let secs = REQUEST_TIMEOUT_SECS.load(Ordering::Relaxed);
        Client::builder()
            .timeout(std::time::Duration::from_secs(secs))
            .build()
            .expect("failed to build shared blocking HTTP client — check TLS/system certificate configuration")
    })
}

/// Shared async HTTP client to avoid per-request allocation.
fn shared_async_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let secs = STREAM_TIMEOUT_SECS.load(Ordering::Relaxed);
        reqwest::Client::builder().timeout(std::time::Duration::from_secs(secs)).build().expect(
            "failed to build shared async HTTP client — check TLS/system certificate configuration",
        )
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSelection {
    pub provider_id: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingRequest {
    pub prompt: String,
    pub required_capabilities: BTreeSet<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_models: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub selected: ModelSelection,
    pub fallback_chain: Vec<ModelSelection>,
    pub reason: String,
}

/// A single part of a multimodal message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    Image { media_type: String, data: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionMessage {
    pub role: String,
    pub content: String,
    /// When non-empty, providers should use these parts instead of `content`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content_parts: Vec<ContentPart>,
}

impl CompletionMessage {
    /// Create a text-only completion message.
    pub fn text(role: impl Into<String>, content: impl Into<String>) -> Self {
        Self { role: role.into(), content: content.into(), content_parts: vec![] }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompletionRequest {
    pub prompt: String,
    /// When non-empty, providers use these parts for the final user message
    /// instead of `prompt` alone (multimodal prompt with images).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub prompt_content_parts: Vec<ContentPart>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub messages: Vec<CompletionMessage>,
    pub required_capabilities: BTreeSet<Capability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompletionResponse {
    pub provider_id: String,
    pub model: String,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallResponse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallResponse {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// A single chunk from a streaming completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionChunk {
    /// Incremental text delta for this chunk.
    pub delta: String,
    /// Set on the final chunk to indicate why generation stopped.
    pub finish_reason: Option<FinishReason>,
    /// Complete tool calls, populated on the final chunk when `finish_reason`
    /// is `ToolCalls`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallResponse>,
}

/// Why the model stopped generating.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
}

/// A boxed stream of completion chunks.
pub type CompletionStream = Pin<Box<dyn Stream<Item = Result<CompletionChunk>> + Send>>;

pub trait ModelProvider: Send + Sync {
    fn descriptor(&self) -> &ProviderDescriptor;

    /// Blocking completion — returns the full response at once.
    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse>;

    /// Streaming completion — yields chunks as they arrive.
    ///
    /// Default implementation wraps `complete()` into a single-chunk stream.
    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream> {
        let response = self.complete(request, selection)?;
        let chunk = CompletionChunk {
            delta: response.content,
            finish_reason: Some(FinishReason::Stop),
            tool_calls: vec![],
        };
        Ok(Box::pin(tokio_stream::once(Ok(chunk))))
    }
}

#[derive(Debug, Clone)]
pub struct EchoProvider {
    descriptor: ProviderDescriptor,
    prefix: String,
}

impl EchoProvider {
    pub fn new(descriptor: ProviderDescriptor, prefix: impl Into<String>) -> Self {
        Self { descriptor, prefix: prefix.into() }
    }
}

impl ModelProvider for EchoProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        let excerpt = request
            .prompt
            .lines()
            .find(|line| !line.trim().is_empty())
            .map(str::trim)
            .unwrap_or("(empty prompt)");
        let excerpt = if excerpt.chars().count() > 200 {
            format!("{}…", excerpt.chars().take(200).collect::<String>())
        } else {
            excerpt.to_string()
        };

        let capability_summary = if request.required_capabilities.is_empty() {
            "general".to_string()
        } else {
            request
                .required_capabilities
                .iter()
                .map(|capability| match capability {
                    Capability::Chat => "chat",
                    Capability::Code => "code",
                    Capability::Vision => "vision",
                    Capability::Embedding => "embedding",
                    Capability::ToolUse => "tool-use",
                })
                .collect::<Vec<_>>()
                .join(", ")
        };

        Ok(CompletionResponse {
            provider_id: self.descriptor.id.clone(),
            model: selection.model.clone(),
            content: format!(
                "{} responded with the `{}` model for a {} request.\n\nPrompt summary: {}",
                self.prefix, selection.model, capability_summary, excerpt
            ),
            tool_calls: vec![],
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderAuth {
    None,
    BearerEnv(String),
    HeaderEnv {
        env_var: String,
        header_name: String,
    },
    /// Look up the API key from the OS keyring using the given key name.
    BearerKeyring {
        key: String,
    },
    /// Look up the API key from the OS keyring and send it as a custom header.
    HeaderKeyring {
        key: String,
        header_name: String,
    },
    GitHubToken,
    /// GitHub Copilot auth: exchanges the OAuth token for a short-lived Copilot session token.
    GitHubCopilotToken,
}

/// Cached Copilot session token with expiry.
#[derive(Clone, Debug)]
struct CopilotTokenCache {
    token: String,
    expires_at: u64,
}

/// Exchange a GitHub OAuth token for a Copilot session token.
fn exchange_copilot_token_blocking(oauth_token: &str) -> Result<CopilotTokenCache> {
    let client = shared_blocking_client();
    let resp = client
        .get("https://api.github.com/copilot_internal/v2/token")
        .header("Authorization", format!("token {oauth_token}"))
        .header("User-Agent", "hivemind-desktop")
        .header("Accept", "application/json")
        .send()
        .context("failed to exchange OAuth token for Copilot session token")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().unwrap_or_default();
        bail!("Copilot token exchange failed ({status}): {body}");
    }

    let body: serde_json::Value = resp.json().context("failed to parse Copilot token response")?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Copilot token response missing 'token' field"))?
        .to_string();
    let expires_at = body["expires_at"].as_u64().unwrap_or_else(|| {
        // Default to 25 minutes from now if not provided
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 1500
    });

    Ok(CopilotTokenCache { token, expires_at })
}

/// Exchange a GitHub OAuth token for a Copilot session token (async).
async fn exchange_copilot_token_async(oauth_token: &str) -> Result<CopilotTokenCache> {
    let client = shared_async_client();
    let resp = client
        .get("https://api.github.com/copilot_internal/v2/token")
        .header("Authorization", format!("token {oauth_token}"))
        .header("User-Agent", "hivemind-desktop")
        .header("Accept", "application/json")
        .send()
        .await
        .context("failed to exchange OAuth token for Copilot session token")?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        bail!("Copilot token exchange failed ({status}): {body}");
    }

    let body: serde_json::Value =
        resp.json().await.context("failed to parse Copilot token response")?;
    let token = body["token"]
        .as_str()
        .ok_or_else(|| anyhow!("Copilot token response missing 'token' field"))?
        .to_string();
    let expires_at = body["expires_at"].as_u64().unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + 1500
    });

    Ok(CopilotTokenCache { token, expires_at })
}

/// Thread-safe cache for Copilot session tokens.
/// Uses `parking_lot::Mutex` which does not poison on panic.
static COPILOT_TOKEN_CACHE: std::sync::LazyLock<parking_lot::Mutex<Option<CopilotTokenCache>>> =
    std::sync::LazyLock::new(|| parking_lot::Mutex::new(None));

fn get_copilot_token_blocking() -> Result<String> {
    let oauth_token = read_keyring("github:oauth-token").or_else(|_| {
        env::var("GITHUB_TOKEN").or_else(|_| env::var("GH_TOKEN")).context(
            "GitHub Copilot requires a saved GitHub token or GITHUB_TOKEN/GH_TOKEN env var",
        )
    })?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    {
        let cache = COPILOT_TOKEN_CACHE.lock();
        if let Some(cached) = cache.as_ref() {
            // Use cached token if it expires more than 60s from now
            if cached.expires_at > now + 60 {
                return Ok(cached.token.clone());
            }
        }
    }

    // If we're inside a tokio runtime, reqwest::blocking panics (nested runtime).
    // Use block_in_place + the async variant instead.
    let new_token = match tokio::runtime::Handle::try_current() {
        Ok(handle) => tokio::task::block_in_place(|| {
            handle.block_on(exchange_copilot_token_async(&oauth_token))
        })?,
        Err(_) => exchange_copilot_token_blocking(&oauth_token)?,
    };
    let token = new_token.token.clone();
    *COPILOT_TOKEN_CACHE.lock() = Some(new_token);
    Ok(token)
}

async fn _get_copilot_token_async() -> Result<String> {
    let oauth_token = read_keyring("github:oauth-token").or_else(|_| {
        env::var("GITHUB_TOKEN").or_else(|_| env::var("GH_TOKEN")).context(
            "GitHub Copilot requires a saved GitHub token or GITHUB_TOKEN/GH_TOKEN env var",
        )
    })?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    {
        let cache = COPILOT_TOKEN_CACHE.lock();
        if let Some(cached) = cache.as_ref() {
            if cached.expires_at > now + 60 {
                return Ok(cached.token.clone());
            }
        }
    }

    let new_token = exchange_copilot_token_async(&oauth_token).await?;
    let token = new_token.token.clone();
    *COPILOT_TOKEN_CACHE.lock() = Some(new_token);
    Ok(token)
}

/// Select the appropriate `ProviderTransport` for the given provider kind.
fn select_transport(kind: &ProviderKind) -> Arc<dyn ProviderTransport> {
    match kind {
        ProviderKind::Anthropic => Arc::new(transport_anthropic::AnthropicTransport),
        ProviderKind::MicrosoftFoundry => Arc::new(transport_foundry::FoundryTransport),
        // OpenAiCompatible, GitHubCopilot, OllamaLocal all use OpenAI wire format.
        // LocalRuntime and Mock are rejected before reaching transport dispatch.
        _ => Arc::new(transport_openai::OpenAiTransport),
    }
}

#[derive(Clone)]
pub struct HttpProvider {
    descriptor: ProviderDescriptor,
    base_url: String,
    auth: ProviderAuth,
    default_api_version: Option<String>,
    extra_headers: BTreeMap<String, String>,
    transport: Arc<dyn ProviderTransport>,
}

impl HttpProvider {
    pub fn new(
        descriptor: ProviderDescriptor,
        base_url: impl Into<String>,
        auth: ProviderAuth,
    ) -> Self {
        let transport = select_transport(&descriptor.kind);
        Self {
            descriptor,
            base_url: base_url.into(),
            auth,
            default_api_version: None,
            extra_headers: BTreeMap::new(),
            transport,
        }
    }

    pub fn with_default_api_version(mut self, api_version: impl Into<String>) -> Self {
        self.default_api_version = Some(api_version.into());
        self
    }

    pub fn with_header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.insert(name.into(), value.into());
        self
    }

    fn transport_context(&self) -> TransportContext<'_> {
        TransportContext {
            base_url: &self.base_url,
            provider_id: &self.descriptor.id,
            provider_kind: &self.descriptor.kind,
            auth: &self.auth,
            extra_headers: &self.extra_headers,
        }
    }
}

impl ModelProvider for HttpProvider {
    fn descriptor(&self) -> &ProviderDescriptor {
        &self.descriptor
    }

    fn complete(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionResponse> {
        let provider = self.clone();
        let request = request.clone();
        let selection = selection.clone();
        std::thread::spawn(move || match provider.descriptor.kind {
            ProviderKind::LocalRuntime => {
                bail!("http provider cannot execute a local-runtime model; use LocalModelProvider")
            }
            ProviderKind::Mock => {
                bail!("http provider cannot execute a provider configured with mock transport")
            }
            _ => {
                let ctx = provider.transport_context();
                provider.transport.complete_blocking(&ctx, &request, &selection)
            }
        })
        .join()
        .map_err(|panic_val| {
            let msg = panic_val
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_val.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            anyhow!("provider {} worker thread panicked: {}", self.descriptor.id, msg)
        })?
    }

    fn complete_stream(
        &self,
        request: &CompletionRequest,
        selection: &ModelSelection,
    ) -> Result<CompletionStream> {
        match self.descriptor.kind {
            ProviderKind::LocalRuntime => {
                bail!("http provider cannot stream a local-runtime model; use LocalModelProvider")
            }
            ProviderKind::Mock => {
                bail!("http provider cannot stream a provider configured with mock transport")
            }
            _ => {
                let ctx = self.transport_context();
                self.transport.complete_stream(&ctx, request, selection)
            }
        }
    }
}

#[derive(Default)]
pub struct ModelRouter {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
}

impl ModelRouter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider<P>(&mut self, provider: P)
    where
        P: ModelProvider + 'static,
    {
        self.providers.insert(provider.descriptor().id.clone(), Arc::new(provider));
    }

    pub fn snapshot(&self) -> ModelRouterSnapshot {
        ModelRouterSnapshot { providers: self.provider_descriptors() }
    }

    /// Returns true if the given provider is a local runtime (loads models into memory).
    pub fn is_local_provider(&self, provider_id: &str) -> bool {
        self.providers
            .get(provider_id)
            .map(|p| matches!(p.descriptor().kind, ProviderKind::LocalRuntime))
            .unwrap_or(false)
    }

    pub fn provider_name(&self, provider_id: &str) -> Option<String> {
        self.providers.get(provider_id).and_then(|provider| provider.descriptor().name.clone())
    }

    pub fn provider_display_name(&self, provider_id: &str) -> String {
        self.provider_name(provider_id).unwrap_or_else(|| provider_id.to_string())
    }

    pub fn provider_descriptors(&self) -> Vec<ProviderDescriptor> {
        let mut providers = self
            .providers
            .values()
            .map(|provider| provider.descriptor().clone())
            .collect::<Vec<_>>();
        providers.sort_by(|left, right| {
            right.priority.cmp(&left.priority).then_with(|| left.id.cmp(&right.id))
        });
        providers
    }

    pub fn route(&self, request: &RoutingRequest) -> Result<RoutingDecision, ModelRouterError> {
        // If the user provided a preference list of model patterns, try each
        // pattern in order against eligible providers.
        // Patterns can be:
        //   "gpt-5.*"           → match model name against all providers
        //   "openai:gpt-5.*"    → match model name only within provider "openai"
        //   "!gpt-5.*-mini"     → exclude models matching this pattern
        if let Some(patterns) = &request.preferred_models {
            let eligible = self.eligible_providers_for_patterns(request);

            tracing::debug!(
                eligible_count = eligible.len(),
                eligible_providers = ?eligible.iter().map(|p| {
                    let d = p.descriptor();
                    format!("{}(models={:?})", d.id, d.models)
                }).collect::<Vec<_>>(),
                patterns = ?patterns,
                "route: preferred_models pattern matching"
            );

            // Partition into positive and exclusion patterns.
            let mut positive_patterns: Vec<&str> = Vec::new();
            let mut exclusion_patterns: Vec<&str> = Vec::new();
            for pattern in patterns {
                if let Some(stripped) = pattern.strip_prefix('!') {
                    exclusion_patterns.push(stripped);
                } else {
                    positive_patterns.push(pattern.as_str());
                }
            }

            let mut ordered: Vec<ModelSelection> = Vec::new();
            let mut matched_pattern: Option<&str> = None;

            for pattern in &positive_patterns {
                let (provider_filter, model_pattern) =
                    if let Some((prov, mdl)) = pattern.split_once(':') {
                        (Some(prov), mdl)
                    } else {
                        (None, *pattern)
                    };

                // Collect matches for this single pattern, then sort by version.
                let mut pattern_matches: Vec<ModelSelection> = Vec::new();

                for provider in &eligible {
                    let descriptor = provider.descriptor();
                    if let Some(pf) = provider_filter {
                        if descriptor.id != pf {
                            continue;
                        }
                    }
                    for model_name in &descriptor.models {
                        if glob_match(model_pattern, model_name) {
                            let model_caps = descriptor.capabilities_for_model(model_name);
                            if !request
                                .required_capabilities
                                .iter()
                                .all(|cap| model_caps.contains(cap))
                            {
                                continue;
                            }
                            let selection = ModelSelection {
                                provider_id: descriptor.id.clone(),
                                model: model_name.clone(),
                            };
                            if !pattern_matches.contains(&selection) {
                                pattern_matches.push(selection);
                            }
                        }
                    }
                }

                // Sort this pattern's matches by version descending.
                // Models without a parseable version sort after versioned ones.
                pattern_matches.sort_by(|a, b| {
                    let va = parse_model_version(&a.model);
                    let vb = parse_model_version(&b.model);
                    match (&vb, &va) {
                        (Some(vb), Some(va)) => vb.cmp(va),
                        (Some(_), None) => std::cmp::Ordering::Greater,
                        (None, Some(_)) => std::cmp::Ordering::Less,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                });

                for selection in pattern_matches {
                    if !ordered.contains(&selection) {
                        if matched_pattern.is_none() {
                            matched_pattern = Some(pattern);
                        }
                        ordered.push(selection);
                    }
                }
            }

            // Apply exclusion patterns: remove any match whose model name
            // matches an exclusion glob.
            if !exclusion_patterns.is_empty() {
                ordered.retain(|sel| {
                    !exclusion_patterns.iter().any(|excl| {
                        let (provider_filter, model_pattern) =
                            if let Some((prov, mdl)) = excl.split_once(':') {
                                (Some(prov), mdl)
                            } else {
                                (None, *excl)
                            };
                        let provider_ok = provider_filter.is_none_or(|pf| sel.provider_id == pf);
                        provider_ok && glob_match(model_pattern, &sel.model)
                    })
                });
            }

            if !ordered.is_empty() {
                let selected = ordered.remove(0);
                return Ok(RoutingDecision {
                    selected,
                    fallback_chain: ordered,
                    reason: format!(
                        "using preferred model pattern: {}",
                        matched_pattern.unwrap_or("?")
                    ),
                });
            }
            tracing::warn!(
                patterns = ?patterns,
                "route: preferred_models patterns matched NO eligible model — falling through to priority routing"
            );
        }

        // Priority-based routing: pick the highest-priority eligible provider
        // and a model that satisfies the required capabilities.
        let eligible = self.eligible_providers(request);

        if eligible.is_empty() {
            return Err(ModelRouterError::NoEligibleProviders);
        }

        let ordered: Vec<_> = eligible
            .iter()
            .filter_map(|provider| {
                capable_selection(provider.descriptor(), &request.required_capabilities)
            })
            .collect();

        let selected = ordered.first().cloned().ok_or(ModelRouterError::NoEligibleProviders)?;

        let fallback_chain = ordered.iter().skip(1).cloned().collect::<Vec<_>>();

        Ok(RoutingDecision {
            selected,
            fallback_chain,
            reason: "using provider priority order".to_string(),
        })
    }

    pub fn complete(
        &self,
        request: &CompletionRequest,
    ) -> Result<CompletionResponse, ModelRouterError> {
        let decision = self.route(&RoutingRequest {
            prompt: request.prompt.clone(),
            required_capabilities: request.required_capabilities.clone(),
            preferred_models: request.preferred_models.clone(),
        })?;
        self.complete_with_decision(request, &decision)
    }

    pub fn complete_with_decision(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
    ) -> Result<CompletionResponse, ModelRouterError> {
        self.complete_with_retry(request, decision, None, &RetryPolicy::default_policy())
    }

    /// Same as [`complete_with_decision`] but accepts an optional callback that
    /// is invoked before each retry sleep, allowing callers to emit events.
    pub fn complete_with_decision_and_callback(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
        retry_callback: Option<&dyn Fn(&RetryInfo)>,
    ) -> Result<CompletionResponse, ModelRouterError> {
        self.complete_with_retry(request, decision, retry_callback, &RetryPolicy::default_policy())
    }

    fn complete_with_retry(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
        retry_callback: Option<&dyn Fn(&RetryInfo)>,
        policy: &RetryPolicy,
    ) -> Result<CompletionResponse, ModelRouterError> {
        let mut attempted = Vec::new();
        let chain: Vec<_> =
            std::iter::once(&decision.selected).chain(decision.fallback_chain.iter()).collect();
        let last_idx = chain.len() - 1;

        let mut last_kind: Option<ProviderErrorKind> = None;
        let mut last_status: Option<u16> = None;

        for (idx, selection) in chain.iter().enumerate() {
            attempted.push(format!("{}:{}", selection.provider_id, selection.model));
            let provider = self.providers.get(&selection.provider_id).ok_or_else(|| {
                ModelRouterError::UnknownProvider { provider_id: selection.provider_id.clone() }
            })?;

            // Same-provider retry loop with exponential backoff.
            for attempt in 0..=policy.max_attempts {
                match provider.complete(request, selection) {
                    Ok(response) => return Ok(response),
                    Err(error) => {
                        let kind = classify_provider_error(&error);
                        let status = extract_http_status(&error);
                        last_kind = Some(kind);
                        last_status = status;

                        if kind.is_retryable() && attempt < policy.max_attempts {
                            let backoff = policy.backoff_ms(attempt);
                            tracing::warn!(
                                provider_id = %selection.provider_id,
                                model = %selection.model,
                                error_kind = ?kind,
                                http_status = ?status,
                                attempt = attempt + 1,
                                max_attempts = policy.max_attempts + 1,
                                backoff_ms = backoff,
                                "transient LLM error, retrying"
                            );
                            if let Some(cb) = retry_callback {
                                cb(&RetryInfo {
                                    provider_id: selection.provider_id.clone(),
                                    model: selection.model.clone(),
                                    attempt: attempt + 1,
                                    max_attempts: policy.max_attempts + 1,
                                    error_kind: kind,
                                    http_status: status,
                                    backoff_ms: backoff,
                                });
                            }
                            std::thread::sleep(std::time::Duration::from_millis(backoff));
                            continue;
                        }

                        if idx == last_idx {
                            return Err(ModelRouterError::ProviderExecutionFailed {
                                attempted,
                                last_error: format!("{:#}", error),
                                error_kind: Some(kind),
                                http_status: status,
                            });
                        }
                        // Non-retryable or retries exhausted: move to next provider.
                        break;
                    }
                }
            }
        }

        Err(ModelRouterError::ProviderExecutionFailed {
            attempted,
            last_error: "all providers failed".to_string(),
            error_kind: last_kind,
            http_status: last_status,
        })
    }

    pub fn complete_stream_with_decision(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
    ) -> Result<(CompletionStream, ModelSelection), ModelRouterError> {
        self.complete_stream_with_retry(request, decision, None, &RetryPolicy::default_policy())
    }

    /// Same as [`complete_stream_with_decision`] but accepts an optional
    /// callback invoked before each retry sleep.
    pub fn complete_stream_with_decision_and_callback(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
        retry_callback: Option<&dyn Fn(&RetryInfo)>,
    ) -> Result<(CompletionStream, ModelSelection), ModelRouterError> {
        self.complete_stream_with_retry(
            request,
            decision,
            retry_callback,
            &RetryPolicy::default_policy(),
        )
    }

    fn complete_stream_with_retry(
        &self,
        request: &CompletionRequest,
        decision: &RoutingDecision,
        retry_callback: Option<&dyn Fn(&RetryInfo)>,
        policy: &RetryPolicy,
    ) -> Result<(CompletionStream, ModelSelection), ModelRouterError> {
        let mut attempted = Vec::new();
        let chain: Vec<_> =
            std::iter::once(&decision.selected).chain(decision.fallback_chain.iter()).collect();
        let last_idx = chain.len() - 1;

        let mut last_kind: Option<ProviderErrorKind> = None;
        let mut last_status: Option<u16> = None;

        for (idx, selection) in chain.iter().enumerate() {
            attempted.push(format!("{}:{}", selection.provider_id, selection.model));
            let provider = self.providers.get(&selection.provider_id).ok_or_else(|| {
                ModelRouterError::UnknownProvider { provider_id: selection.provider_id.clone() }
            })?;

            // Same-provider retry loop with exponential backoff.
            for attempt in 0..=policy.max_attempts {
                match provider.complete_stream(request, selection) {
                    Ok(stream) => return Ok((stream, (*selection).clone())),
                    Err(error) => {
                        let kind = classify_provider_error(&error);
                        let status = extract_http_status(&error);
                        last_kind = Some(kind);
                        last_status = status;

                        if kind.is_retryable() && attempt < policy.max_attempts {
                            let backoff = policy.backoff_ms(attempt);
                            tracing::warn!(
                                provider_id = %selection.provider_id,
                                model = %selection.model,
                                error_kind = ?kind,
                                http_status = ?status,
                                attempt = attempt + 1,
                                max_attempts = policy.max_attempts + 1,
                                backoff_ms = backoff,
                                "transient streaming LLM error, retrying"
                            );
                            if let Some(cb) = retry_callback {
                                cb(&RetryInfo {
                                    provider_id: selection.provider_id.clone(),
                                    model: selection.model.clone(),
                                    attempt: attempt + 1,
                                    max_attempts: policy.max_attempts + 1,
                                    error_kind: kind,
                                    http_status: status,
                                    backoff_ms: backoff,
                                });
                            }
                            // Streaming setup is blocking (only the stream consumption is async),
                            // so we use std::thread::sleep here.
                            std::thread::sleep(std::time::Duration::from_millis(backoff));
                            continue;
                        }

                        if idx == last_idx || !kind.is_retryable() {
                            return Err(ModelRouterError::ProviderExecutionFailed {
                                attempted,
                                last_error: format!("{:#}", error),
                                error_kind: Some(kind),
                                http_status: status,
                            });
                        }
                        tracing::warn!(
                            provider_id = %selection.provider_id,
                            model = %selection.model,
                            error_kind = ?kind,
                            "streaming call failed after retries, trying next in fallback chain"
                        );
                        // Retries exhausted but retryable: move to next provider.
                        break;
                    }
                }
            }
        }

        Err(ModelRouterError::ProviderExecutionFailed {
            attempted,
            last_error: "all providers failed".to_string(),
            error_kind: last_kind,
            http_status: last_status,
        })
    }

    fn eligible_providers<'a>(
        &'a self,
        request: &RoutingRequest,
    ) -> Vec<&'a Arc<dyn ModelProvider>> {
        let mut providers = self
            .providers
            .values()
            .filter(|provider| {
                let descriptor = provider.descriptor();
                if !descriptor.available {
                    return false;
                }
                // A provider is eligible if at least one of its models
                // satisfies all required capabilities (checking per-model
                // overrides when present).
                descriptor.models.iter().any(|model| {
                    let caps = descriptor.capabilities_for_model(model);
                    request.required_capabilities.iter().all(|cap| caps.contains(cap))
                })
            })
            .collect::<Vec<_>>();

        providers
            .sort_by(|left, right| right.descriptor().priority.cmp(&left.descriptor().priority));
        providers
    }

    /// Eligible providers for user-specified model patterns.
    /// Only filters by availability — per-model capability checks are deferred
    /// to the pattern-matching loop in `route()` so that providers with
    /// per-model capability overrides are not prematurely excluded.
    fn eligible_providers_for_patterns<'a>(
        &'a self,
        _request: &RoutingRequest,
    ) -> Vec<&'a Arc<dyn ModelProvider>> {
        let mut providers = self
            .providers
            .values()
            .filter(|provider| provider.descriptor().available)
            .collect::<Vec<_>>();

        providers
            .sort_by(|left, right| right.descriptor().priority.cmp(&left.descriptor().priority));
        providers
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ModelRouterError {
    #[error("no eligible providers")]
    NoEligibleProviders,
    #[error("unknown provider {provider_id}")]
    UnknownProvider { provider_id: String },
    #[error("unknown model {model} on provider {provider_id}")]
    UnknownModel { provider_id: String, model: String },
    #[error("preferred model on provider {provider_id} is unavailable")]
    PreferredModelUnavailable { provider_id: String },
    #[error("preferred model on provider {provider_id} does not satisfy required capabilities")]
    PreferredModelMissingCapabilities { provider_id: String },
    #[error("provider execution failed after trying {attempted:?}: {last_error}")]
    ProviderExecutionFailed {
        attempted: Vec<String>,
        last_error: String,
        /// Classified error kind for the final failure.
        error_kind: Option<ProviderErrorKind>,
        /// HTTP status code from the final failure, if available.
        http_status: Option<u16>,
    },
}

/// Structured provider error carrying HTTP status, classification, and context.
#[derive(Debug, Clone, Error)]
#[error("provider {provider_id} error: {message}")]
pub struct ProviderError {
    pub provider_id: String,
    pub model: Option<String>,
    pub kind: ProviderErrorKind,
    pub http_status: Option<u16>,
    pub message: String,
}

/// Classifies a provider error as retryable or not.
/// Retryable errors (rate limiting, transient server errors) can be tried on
/// the next provider in the fallback chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderErrorKind {
    /// HTTP 429 — Too Many Requests / rate limited.
    RateLimited,
    /// HTTP 500, 502, 503, 504 — transient server errors.
    ServerError,
    /// HTTP 401, 403 — authentication / authorization failures.
    AuthError,
    /// HTTP 402 — insufficient credits / payment required.
    InsufficientCredits,
    /// Any other error (network, parse, unknown).
    Other,
}

impl ProviderErrorKind {
    /// Whether this error kind is worth retrying on a different provider.
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited | Self::ServerError)
    }
}

/// Classify a provider error. If the error is a [`ProviderError`], the kind is
/// returned directly; otherwise the error message is inspected for HTTP status codes.
pub fn classify_provider_error(error: &anyhow::Error) -> ProviderErrorKind {
    if let Some(pe) = error.downcast_ref::<ProviderError>() {
        return pe.kind;
    }
    let msg = error.to_string();
    classify_error_message(&msg)
}

/// Extract the HTTP status code from an error, if available.
pub fn extract_http_status(error: &anyhow::Error) -> Option<u16> {
    if let Some(pe) = error.downcast_ref::<ProviderError>() {
        return pe.http_status;
    }
    extract_status_from_message(&error.to_string())
}

/// Extract an HTTP status code from an error message string.
fn extract_status_from_message(msg: &str) -> Option<u16> {
    if let Some(pos) = msg.find("returned ") {
        let after = &msg[pos + "returned ".len()..];
        if let Some(code_str) = after.split(|c: char| !c.is_ascii_digit()).next() {
            if let Ok(status) = code_str.parse::<u16>() {
                return Some(status);
            }
        }
    }
    None
}

/// Classify an error from its message string.
fn classify_error_message(msg: &str) -> ProviderErrorKind {
    // Check for credit/billing keywords in the error body (providers like Anthropic
    // return 400 instead of 402 for credit exhaustion).
    let lower = msg.to_lowercase();
    if lower.contains("credit balance")
        || lower.contains("insufficient credits")
        || lower.contains("purchase credits")
        || lower.contains("billing") && (lower.contains("upgrade") || lower.contains("too low"))
    {
        return ProviderErrorKind::InsufficientCredits;
    }
    // Look for "returned <status_code>:" pattern
    if let Some(pos) = msg.find("returned ") {
        let after = &msg[pos + "returned ".len()..];
        if let Some(code_str) = after.split(|c: char| !c.is_ascii_digit()).next() {
            if let Ok(status) = code_str.parse::<u16>() {
                return match status {
                    429 => ProviderErrorKind::RateLimited,
                    402 => ProviderErrorKind::InsufficientCredits,
                    401 | 403 => ProviderErrorKind::AuthError,
                    500 | 502..=504 => ProviderErrorKind::ServerError,
                    _ => ProviderErrorKind::Other,
                };
            }
        }
    }
    // Connection failures are generally retryable
    if msg.contains("failed to contact provider") {
        return ProviderErrorKind::ServerError;
    }
    ProviderErrorKind::Other
}

/// Simple glob matching against model names.
/// Supports `*` (matches any sequence of characters) and `?` (matches exactly one character).
pub fn glob_match(pattern: &str, text: &str) -> bool {
    fn inner(pattern: &[char], text: &[char]) -> bool {
        let (mut p, mut t) = (0, 0);
        let (mut star_p, mut star_t) = (usize::MAX, 0);

        while t < text.len() {
            if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
                p += 1;
                t += 1;
            } else if p < pattern.len() && pattern[p] == '*' {
                star_p = p;
                star_t = t;
                p += 1;
            } else if star_p != usize::MAX {
                p = star_p + 1;
                star_t += 1;
                t = star_t;
            } else {
                return false;
            }
        }

        while p < pattern.len() && pattern[p] == '*' {
            p += 1;
        }
        p == pattern.len()
    }

    let pc: Vec<char> = pattern.chars().collect();
    let tc: Vec<char> = text.chars().collect();
    inner(&pc, &tc)
}

/// Extract version components from a model name for sorting.
///
/// Finds the *best* version-like sequence (`N.N.N…` or standalone `N`) in the
/// model name.  "Best" means the longest dot-separated numeric run.
///
/// Examples:
///   `"claude-sonnet-4.6"` → `Some([4, 6])`
///   `"gpt-5.2"`           → `Some([5, 2])`
///   `"gpt-5.2-codex"`     → `Some([5, 2])`
///   `"phi-3-mini"`         → `Some([3])`
///   `"bge-small-en-v1.5"`  → `Some([1, 5])`
///   `"my-model"`           → `None`
pub fn parse_model_version(name: &str) -> Option<Vec<u32>> {
    // Strategy: scan for all maximal runs of `\d+(\.\d+)*` and pick the longest.
    let bytes = name.as_bytes();
    let len = bytes.len();
    let mut best: Option<Vec<u32>> = None;

    let mut i = 0;
    while i < len {
        // Skip until we find a digit that is either at the start or preceded by
        // a non-alphanumeric character (so we don't grab digits embedded inside
        // words like "v1" → we DO want that).
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }

        // Parse a dotted-numeric run: N(.N)*
        let mut components: Vec<u32> = Vec::new();
        loop {
            let start = i;
            while i < len && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if let Ok(n) = name[start..i].parse::<u32>() {
                components.push(n);
            } else {
                break;
            }
            // Continue if followed by `.digit`
            if i < len && bytes[i] == b'.' && i + 1 < len && bytes[i + 1].is_ascii_digit() {
                i += 1; // skip the dot
            } else {
                break;
            }
        }

        if !components.is_empty() {
            let is_better = match &best {
                None => true,
                Some(prev) => {
                    components.len() > prev.len()
                        || (components.len() == prev.len() && components > *prev)
                }
            };
            if is_better {
                best = Some(components);
            }
        }
    }

    best
}

fn format_tools_openai(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
    if tools.is_empty() {
        return None;
    }
    Some(
        tools
            .iter()
            .map(|t| {
                // OpenAI function names must match ^[a-zA-Z0-9_-]{1,64}$
                let name = sanitize_tool_name(&t.id);
                let mut params = t.input_schema.clone();
                sanitize_openai_schema(&mut params);
                json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.description,
                        "parameters": params
                    }
                })
            })
            .collect(),
    )
}

/// Recursively sanitize a JSON Schema value so that it conforms to the
/// strict subset accepted by OpenAI-compatible APIs (including GitHub
/// Copilot).  Fixes:
/// - `type` given as an array → pick the first concrete type
/// - `type: "array"` without `items` → add `items: { type: "string" }`
/// - `type: "object"` without `properties` → add empty `properties`
/// - missing `type` on property definitions → add `type: "string"`
/// - `additionalProperties` on objects (set to `false` when missing)
/// - strip unsupported keywords (`default`, `examples`, `$schema`, `$id`,
///   `title`, `$comment`)
fn sanitize_openai_schema(value: &mut serde_json::Value) {
    sanitize_openai_schema_inner(value, 0);
}

fn sanitize_openai_schema_inner(value: &mut serde_json::Value, depth: usize) {
    const MAX_SCHEMA_DEPTH: usize = 64;
    if depth > MAX_SCHEMA_DEPTH {
        return;
    }

    let obj = match value.as_object_mut() {
        Some(o) => o,
        None => return,
    };

    // 0. Remove keywords that OpenAI / GitHub Copilot do not accept.
    const STRIP_KEYS: &[&str] = &["default", "examples", "$schema", "$id", "title", "$comment"];
    for key in STRIP_KEYS {
        obj.remove(*key);
    }

    // 1. Normalise `type` — if it is an array, pick the first non-null type.
    if let Some(type_val) = obj.get("type").cloned() {
        if let Some(arr) = type_val.as_array() {
            let first = arr
                .iter()
                .find(|v| v.as_str() != Some("null"))
                .or_else(|| arr.first())
                .cloned()
                .unwrap_or_else(|| json!("string"));
            obj.insert("type".to_string(), first);
        }
    }

    let current_type = obj.get("type").and_then(|v| v.as_str()).map(String::from);

    // 2. Arrays MUST have `items`.
    if current_type.as_deref() == Some("array") && !obj.contains_key("items") {
        obj.insert("items".to_string(), json!({ "type": "string" }));
    }

    // 3. Objects MUST have `properties` (even if empty).
    if current_type.as_deref() == Some("object") && !obj.contains_key("properties") {
        obj.insert("properties".to_string(), serde_json::Value::Object(serde_json::Map::new()));
    }

    // 4. Objects MUST have `additionalProperties` set.
    //    GitHub Copilot (and OpenAI strict mode) requires this field.
    //    If it is missing, default to `false`.
    if current_type.as_deref() == Some("object") && !obj.contains_key("additionalProperties") {
        obj.insert("additionalProperties".to_string(), json!(false));
    }

    // 5. Recurse into `properties`.
    if let Some(props) = obj.get_mut("properties") {
        if let Some(props_map) = props.as_object_mut() {
            for (_key, prop_schema) in props_map.iter_mut() {
                if let Some(prop_obj) = prop_schema.as_object_mut() {
                    // Ensure every property has a `type`.
                    if !prop_obj.contains_key("type") {
                        prop_obj.insert("type".to_string(), json!("string"));
                    }
                }
                sanitize_openai_schema_inner(prop_schema, depth + 1);
            }
        }
    }

    // 6. Recurse into `items`.
    if let Some(items) = obj.get_mut("items") {
        sanitize_openai_schema_inner(items, depth + 1);
    }

    // 7. Recurse into `additionalProperties` if it is a schema object.
    if let Some(addl) = obj.get_mut("additionalProperties") {
        if addl.is_object() {
            sanitize_openai_schema_inner(addl, depth + 1);
        }
    }
}

/// Sanitize a tool name for OpenAI-compatible APIs.
/// Replaces any character not in [a-zA-Z0-9_-] with an underscore.
pub fn sanitize_tool_name(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .collect::<String>()
        .chars()
        .take(64)
        .collect()
}

/// Restore tool names that were sanitized for the OpenAI wire format.
/// Builds a reverse mapping from the actual tool definitions so we don't
/// rely on fragile heuristics.
fn build_tool_name_map(tools: &[ToolDefinition]) -> HashMap<String, String> {
    tools.iter().map(|t| (sanitize_tool_name(&t.id), t.id.clone())).collect()
}

/// Look up the original tool name from a sanitized OpenAI function name.
fn restore_tool_name_with_map(sanitized: &str, map: &HashMap<String, String>) -> String {
    map.get(sanitized).cloned().unwrap_or_else(|| sanitized.to_string())
}

/// Fallback restore for contexts where the tool map is unavailable.
fn format_tools_anthropic(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
    if tools.is_empty() {
        return None;
    }
    // Anthropic tool names must match ^[a-zA-Z0-9_-]{1,128}$
    Some(
        tools
            .iter()
            .map(|t| {
                let name = sanitize_tool_name(&t.id);
                json!({
                    "name": name,
                    "description": t.description,
                    "input_schema": t.input_schema
                })
            })
            .collect(),
    )
}

#[allow(dead_code)] // Reserved for future Ollama-specific tool formatting
fn format_tools_ollama(tools: &[ToolDefinition]) -> Option<Vec<serde_json::Value>> {
    if tools.is_empty() {
        return None;
    }
    // Ollama uses OpenAI-compatible format but is more lenient with names.
    // Still sanitize to be safe since it proxies to various model backends.
    Some(
        tools
            .iter()
            .map(|t| {
                let name = sanitize_tool_name(&t.id);
                let mut params = t.input_schema.clone();
                sanitize_openai_schema(&mut params);
                json!({
                    "type": "function",
                    "function": {
                        "name": name,
                        "description": t.description,
                        "parameters": params
                    }
                })
            })
            .collect(),
    )
}

fn openai_messages_from_request(request: &CompletionRequest) -> Vec<OpenAiMessage> {
    let mut messages = request
        .messages
        .iter()
        .map(|message| OpenAiMessage {
            role: message.role.clone(),
            content: openai_content_from_completion(message),
        })
        .collect::<Vec<_>>();
    // Final user message: use prompt_content_parts when present (multimodal).
    let prompt_content = if request.prompt_content_parts.is_empty() {
        OpenAiContent::Text(request.prompt.clone())
    } else {
        OpenAiContent::Parts(
            request
                .prompt_content_parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => OpenAiContentPart::Text { text: text.clone() },
                    ContentPart::Image { media_type, data } => OpenAiContentPart::ImageUrl {
                        image_url: OpenAiImageUrl {
                            url: format!("data:{media_type};base64,{data}"),
                        },
                    },
                })
                .collect(),
        )
    };
    messages.push(OpenAiMessage { role: "user".to_string(), content: prompt_content });
    messages
}

fn anthropic_messages_from_request(request: &CompletionRequest) -> Vec<AnthropicMessage> {
    let mut messages = request
        .messages
        .iter()
        .map(|message| AnthropicMessage {
            role: message.role.clone(),
            content: anthropic_content_from_completion(message),
        })
        .collect::<Vec<_>>();
    // Final user message: use prompt_content_parts when present (multimodal).
    let prompt_content = if request.prompt_content_parts.is_empty() {
        AnthropicContent::Text(request.prompt.clone())
    } else {
        AnthropicContent::Parts(
            request
                .prompt_content_parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => AnthropicContentPart::Text { text: text.clone() },
                    ContentPart::Image { media_type, data } => AnthropicContentPart::Image {
                        source: AnthropicImageSource {
                            source_type: "base64".to_string(),
                            media_type: media_type.clone(),
                            data: data.clone(),
                        },
                    },
                })
                .collect(),
        )
    };
    messages.push(AnthropicMessage { role: "user".to_string(), content: prompt_content });
    messages
}

/// Convert a `CompletionMessage` to OpenAI content format.
fn openai_content_from_completion(msg: &CompletionMessage) -> OpenAiContent {
    if msg.content_parts.is_empty() {
        return OpenAiContent::Text(msg.content.clone());
    }
    OpenAiContent::Parts(
        msg.content_parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => OpenAiContentPart::Text { text: text.clone() },
                ContentPart::Image { media_type, data } => OpenAiContentPart::ImageUrl {
                    image_url: OpenAiImageUrl { url: format!("data:{media_type};base64,{data}") },
                },
            })
            .collect(),
    )
}

/// Convert a `CompletionMessage` to Anthropic content format.
fn anthropic_content_from_completion(msg: &CompletionMessage) -> AnthropicContent {
    if msg.content_parts.is_empty() {
        return AnthropicContent::Text(msg.content.clone());
    }
    AnthropicContent::Parts(
        msg.content_parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => AnthropicContentPart::Text { text: text.clone() },
                ContentPart::Image { media_type, data } => AnthropicContentPart::Image {
                    source: AnthropicImageSource {
                        source_type: "base64".to_string(),
                        media_type: media_type.clone(),
                        data: data.clone(),
                    },
                },
            })
            .collect(),
    )
}

#[derive(Debug, Serialize)]
struct OpenAiChatRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    content: OpenAiContent,
}

/// OpenAI message content: either a plain string or an array of content parts.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum OpenAiContent {
    Text(String),
    Parts(Vec<OpenAiContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum OpenAiContentPart {
    Text { text: String },
    ImageUrl { image_url: OpenAiImageUrl },
}

#[derive(Debug, Serialize)]
struct OpenAiImageUrl {
    url: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiChatResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: Option<OpenAiChoiceMessage>,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoiceMessage {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiToolCallItem>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallItem {
    id: Option<String>,
    function: Option<OpenAiToolCallFunction>,
}

#[derive(Debug, Deserialize)]
struct OpenAiToolCallFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: AnthropicContent,
}

/// Anthropic message content: either a plain string or an array of content blocks.
#[derive(Debug, Serialize)]
#[serde(untagged)]
enum AnthropicContent {
    Text(String),
    Parts(Vec<AnthropicContentPart>),
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentPart {
    Text { text: String },
    Image { source: AnthropicImageSource },
}

#[derive(Debug, Serialize)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    source_type: String,
    media_type: String,
    data: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Debug, Deserialize)]
struct AnthropicBlock {
    #[serde(rename = "type")]
    block_type: Option<String>,
    text: Option<String>,
    id: Option<String>,
    name: Option<String>,
    input: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// SSE streaming request/response structs
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct OpenAiChatStreamRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Serialize)]
struct AnthropicStreamRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<AnthropicMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<serde_json::Value>>,
}

/// A single SSE chunk from an OpenAI-compatible streaming response.
#[derive(Debug, Deserialize)]
struct OpenAiStreamChunk {
    choices: Vec<OpenAiStreamChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamChoice {
    delta: Option<OpenAiStreamDelta>,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAiStreamToolCallDelta>>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    function: Option<OpenAiStreamToolCallFnDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAiStreamToolCallFnDelta {
    name: Option<String>,
    arguments: Option<String>,
}

fn read_env(env_var: &str) -> Result<String> {
    env::var(env_var)
        .with_context(|| format!("required provider credential env var `{env_var}` is not set"))
}

fn read_keyring(key: &str) -> Result<String> {
    hive_core::secret_store::load(key)
        .with_context(|| format!("no secret found in OS keyring for key `{key}`"))
}

/// Evicts a single key from the keyring cache.
/// Call this whenever a secret is saved or deleted via the UI.
pub fn invalidate_keyring_cache_entry(key: &str) {
    hive_core::secret_cache::invalidate_cached_secret(key);
}

/// Evicts all entries from the keyring cache.
/// Useful on config reload or provider re-initialization.
pub fn invalidate_keyring_cache() {
    hive_core::secret_cache::invalidate_all_cached_secrets();
}

fn trim_trailing_slash(base_url: &str) -> &str {
    base_url.trim_end_matches('/')
}

/// Partial tool call data extracted from a single SSE chunk.
#[derive(Debug)]
pub(crate) enum ToolCallDelta {
    /// OpenAI: a tool_calls delta with an index, optional id/name, and optional args fragment.
    OpenAi { index: usize, id: Option<String>, name: Option<String>, arguments: Option<String> },
    /// Anthropic: start of a tool_use content block.
    AnthropicStart { id: String, name: String },
    /// Anthropic: partial JSON for a tool_use content block.
    AnthropicArgsDelta { partial_json: String },
    /// Anthropic: end of a content block (finalize current entry).
    AnthropicStop,
}

/// Result from parsing a single SSE data line, carrying both the user-visible
/// chunk and any tool-call deltas that need accumulation.
pub(crate) struct SseParseResult {
    pub(crate) chunk: Option<CompletionChunk>,
    pub(crate) tool_call_deltas: Vec<ToolCallDelta>,
}

fn parse_sse_data(data: &str, kind: &ProviderKind, provider_id: &str) -> Result<SseParseResult> {
    match kind {
        ProviderKind::Anthropic => parse_anthropic_sse_data(data, provider_id),
        // All others (OpenAI-compatible, Azure, GitHub Copilot, Ollama, MicrosoftFoundry) use
        // OpenAI-style streaming JSON.
        _ => parse_openai_sse_data(data, provider_id),
    }
}

fn parse_openai_sse_data(data: &str, provider_id: &str) -> Result<SseParseResult> {
    let chunk: OpenAiStreamChunk = serde_json::from_str(data)
        .with_context(|| format!("provider {provider_id} returned malformed streaming json"))?;
    let choice = match chunk.choices.into_iter().next() {
        Some(c) => c,
        None => return Ok(SseParseResult { chunk: None, tool_call_deltas: vec![] }),
    };

    let mut tool_call_deltas = Vec::new();
    let mut delta_text = String::new();

    if let Some(delta) = choice.delta {
        if let Some(content) = delta.content {
            delta_text = content;
        }
        if let Some(tc_deltas) = delta.tool_calls {
            for tcd in tc_deltas {
                let index = tcd.index.unwrap_or(0);
                let (fn_name, fn_args) = match tcd.function {
                    Some(f) => (f.name, f.arguments),
                    None => (None, None),
                };
                tool_call_deltas.push(ToolCallDelta::OpenAi {
                    index,
                    id: tcd.id,
                    name: fn_name,
                    arguments: fn_args,
                });
            }
        }
    }

    let finish_reason = choice.finish_reason.as_deref().and_then(parse_finish_reason);

    let chunk = if delta_text.is_empty() && finish_reason.is_none() && tool_call_deltas.is_empty() {
        None
    } else {
        Some(CompletionChunk { delta: delta_text, finish_reason, tool_calls: vec![] })
    };

    Ok(SseParseResult { chunk, tool_call_deltas })
}

fn parse_anthropic_sse_data(data: &str, provider_id: &str) -> Result<SseParseResult> {
    let value: serde_json::Value = serde_json::from_str(data)
        .with_context(|| format!("provider {provider_id} returned malformed streaming json"))?;

    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        "content_block_start" => {
            let block = value.get("content_block");
            let block_type = block.and_then(|b| b.get("type")).and_then(|t| t.as_str());
            if block_type == Some("tool_use") {
                let id = block
                    .and_then(|b| b.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let name = block
                    .and_then(|b| b.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(SseParseResult {
                    chunk: None,
                    tool_call_deltas: vec![ToolCallDelta::AnthropicStart { id, name }],
                });
            }
            Ok(SseParseResult { chunk: None, tool_call_deltas: vec![] })
        }
        "content_block_delta" => {
            let delta = value.get("delta");
            let delta_type = delta.and_then(|d| d.get("type")).and_then(|t| t.as_str());

            if delta_type == Some("input_json_delta") {
                let partial = delta
                    .and_then(|d| d.get("partial_json"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                return Ok(SseParseResult {
                    chunk: None,
                    tool_call_deltas: vec![ToolCallDelta::AnthropicArgsDelta {
                        partial_json: partial,
                    }],
                });
            }

            // Text delta
            let delta_text =
                delta.and_then(|d| d.get("text")).and_then(|t| t.as_str()).unwrap_or("");
            if delta_text.is_empty() {
                return Ok(SseParseResult { chunk: None, tool_call_deltas: vec![] });
            }
            Ok(SseParseResult {
                chunk: Some(CompletionChunk {
                    delta: delta_text.to_string(),
                    finish_reason: None,
                    tool_calls: vec![],
                }),
                tool_call_deltas: vec![],
            })
        }
        "content_block_stop" => {
            Ok(SseParseResult { chunk: None, tool_call_deltas: vec![ToolCallDelta::AnthropicStop] })
        }
        "message_delta" => {
            let stop_reason =
                value.get("delta").and_then(|d| d.get("stop_reason")).and_then(|r| r.as_str());
            let finish_reason = stop_reason.and_then(|r| match r {
                "end_turn" | "stop" => Some(FinishReason::Stop),
                "max_tokens" => Some(FinishReason::Length),
                "tool_use" => Some(FinishReason::ToolCalls),
                _ => None,
            });
            if finish_reason.is_some() {
                Ok(SseParseResult {
                    chunk: Some(CompletionChunk {
                        delta: String::new(),
                        finish_reason,
                        tool_calls: vec![],
                    }),
                    tool_call_deltas: vec![],
                })
            } else {
                Ok(SseParseResult { chunk: None, tool_call_deltas: vec![] })
            }
        }
        _ => Ok(SseParseResult { chunk: None, tool_call_deltas: vec![] }),
    }
}

fn parse_finish_reason(reason: &str) -> Option<FinishReason> {
    match reason {
        "stop" => Some(FinishReason::Stop),
        "length" => Some(FinishReason::Length),
        "tool_calls" | "function_call" => Some(FinishReason::ToolCalls),
        _ => None,
    }
}

/// Pick the first model from a provider that satisfies the required capabilities.
fn capable_selection(
    descriptor: &ProviderDescriptor,
    required: &BTreeSet<Capability>,
) -> Option<ModelSelection> {
    descriptor
        .models
        .iter()
        .find(|model| {
            let caps = descriptor.capabilities_for_model(model);
            required.iter().all(|cap| caps.contains(cap))
        })
        .map(|model| ModelSelection { provider_id: descriptor.id.clone(), model: model.clone() })
}

// ---------------------------------------------------------------------------
// Local-model provider (delegates to hive-inference runtime manager)
// ---------------------------------------------------------------------------
// Moved to local_provider.rs — re-exported at top of this file.

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    struct StaticProvider {
        descriptor: ProviderDescriptor,
        content: String,
        fail: bool,
    }

    impl StaticProvider {
        fn new(id: &str, capabilities: &[Capability], priority: i32, content: &str) -> Self {
            let models = vec!["default".to_string()];
            Self {
                descriptor: ProviderDescriptor {
                    id: id.to_string(),
                    name: None,
                    kind: ProviderKind::Mock,
                    model_capabilities: BTreeMap::from([(
                        "default".to_string(),
                        capabilities.iter().cloned().collect(),
                    )]),
                    models,
                    priority,
                    available: true,
                },
                content: content.to_string(),
                fail: false,
            }
        }

        fn failing(mut self) -> Self {
            self.fail = true;
            self
        }

        fn with_models(mut self, models: Vec<String>) -> Self {
            // Grab the capabilities from the first existing model entry (the
            // "default" model set in `new`), then rebuild the map for the new
            // model list.
            let caps =
                self.descriptor.model_capabilities.values().next().cloned().unwrap_or_default();
            self.descriptor.model_capabilities =
                models.iter().map(|m| (m.clone(), caps.clone())).collect();
            self.descriptor.models = models;
            self
        }
    }

    impl ModelProvider for StaticProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn complete(
            &self,
            _request: &CompletionRequest,
            selection: &ModelSelection,
        ) -> Result<CompletionResponse> {
            if self.fail {
                bail!("provider failed");
            }

            Ok(CompletionResponse {
                provider_id: self.descriptor.id.clone(),
                model: selection.model.clone(),
                content: self.content.clone(),
                tool_calls: vec![],
            })
        }
    }

    fn basic_request() -> RoutingRequest {
        RoutingRequest {
            prompt: "hello".to_string(),
            required_capabilities: [Capability::Chat].into_iter().collect(),
            preferred_models: None,
        }
    }

    #[test]
    fn routes_to_highest_priority_provider() {
        let mut router = ModelRouter::new();
        router.register_provider(StaticProvider::new("openai", &[Capability::Chat], 100, "public"));
        router.register_provider(StaticProvider::new("local", &[Capability::Chat], 50, "private"));

        let decision = router.route(&basic_request()).expect("route");

        assert_eq!(decision.selected.provider_id, "openai");
    }

    #[test]
    fn preferred_model_pattern_selects_matching_model() {
        let mut router = ModelRouter::new();
        router.register_provider(StaticProvider::new(
            "openrouter",
            &[Capability::Chat],
            100,
            "public",
        ));

        let request = RoutingRequest {
            preferred_models: Some(vec!["openrouter:default".to_string()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("pattern pick should succeed");
        assert_eq!(decision.selected.provider_id, "openrouter");
        assert_eq!(decision.selected.model, "default");
        assert!(decision.reason.starts_with("using preferred model pattern"));
    }

    #[test]
    fn glob_match_exact() {
        assert!(glob_match("gpt-5.2", "gpt-5.2"));
        assert!(!glob_match("gpt-5.2", "gpt-5.3"));
    }

    #[test]
    fn glob_match_star_suffix() {
        assert!(glob_match("gpt-5.*", "gpt-5.2"));
        assert!(glob_match("gpt-5.*", "gpt-5.2-turbo"));
        assert!(!glob_match("gpt-5.*", "gpt-4.1"));
    }

    #[test]
    fn glob_match_star_prefix() {
        assert!(glob_match("*turbo", "gpt-5.2-turbo"));
        assert!(!glob_match("*turbo", "gpt-5.2-mini"));
    }

    #[test]
    fn glob_match_question_mark() {
        assert!(glob_match("gpt-5.?", "gpt-5.2"));
        assert!(!glob_match("gpt-5.?", "gpt-5.22"));
    }

    #[test]
    fn glob_match_star_middle() {
        assert!(glob_match("claude-*-4.5", "claude-sonnet-4.5"));
        assert!(!glob_match("claude-*-4.5", "claude-sonnet-4.6"));
    }

    #[test]
    fn glob_match_empty_and_star() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*", ""));
        assert!(!glob_match("a", ""));
        assert!(glob_match("", ""));
    }

    // ── parse_model_version tests ──────────────────────────────────

    #[test]
    fn version_simple_major_minor() {
        assert_eq!(parse_model_version("claude-sonnet-4.6"), Some(vec![4, 6]));
        assert_eq!(parse_model_version("gpt-5.2"), Some(vec![5, 2]));
    }

    #[test]
    fn version_with_suffix() {
        assert_eq!(parse_model_version("gpt-5.2-codex"), Some(vec![5, 2]));
        assert_eq!(parse_model_version("gpt-5.4-mini"), Some(vec![5, 4]));
    }

    #[test]
    fn version_single_number() {
        assert_eq!(parse_model_version("phi-3-mini"), Some(vec![3]));
    }

    #[test]
    fn version_with_v_prefix() {
        assert_eq!(parse_model_version("bge-small-en-v1.5"), Some(vec![1, 5]));
    }

    #[test]
    fn version_three_components() {
        assert_eq!(parse_model_version("model-1.2.3"), Some(vec![1, 2, 3]));
    }

    #[test]
    fn version_picks_longest_run() {
        // "3" is a single-component run, "4.6" is a two-component run → pick 4.6
        assert_eq!(parse_model_version("v3-claude-sonnet-4.6"), Some(vec![4, 6]));
    }

    #[test]
    fn version_no_version() {
        assert_eq!(parse_model_version("my-custom-model"), None);
        assert_eq!(parse_model_version("default"), None);
    }

    #[test]
    fn version_comparison_ordering() {
        let mut versions = vec![
            ("gpt-5.1", parse_model_version("gpt-5.1")),
            ("gpt-5.4", parse_model_version("gpt-5.4")),
            ("gpt-5.2", parse_model_version("gpt-5.2")),
        ];
        versions.sort_by(|a, b| b.1.cmp(&a.1));
        let names: Vec<&str> = versions.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["gpt-5.4", "gpt-5.2", "gpt-5.1"]);
    }

    #[test]
    fn pattern_list_first_match_wins() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.1".into(), "gpt-5.2".into()]),
        );
        router.register_provider(
            StaticProvider::new("anthropic", &[Capability::Chat], 90, "anth")
                .with_models(vec!["claude-sonnet-4.5".into(), "claude-sonnet-4.6".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["claude-sonnet-4.*".into(), "gpt-5.*".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        // First pattern matches claude models, version-sorted (4.6 before 4.5)
        assert_eq!(decision.selected.provider_id, "anthropic");
        assert_eq!(decision.selected.model, "claude-sonnet-4.6");
        // Remaining matches form fallback chain
        assert!(decision.fallback_chain.len() >= 2);
    }

    #[test]
    fn pattern_no_match_falls_back_to_normal_routing() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.2".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["nonexistent-model-*".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("should fall back to normal routing");
        assert_eq!(decision.selected.provider_id, "openai");
        assert_eq!(decision.reason, "using provider priority order");
    }

    #[test]
    fn pattern_with_provider_prefix_scopes_to_provider() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.2".into()]),
        );
        router.register_provider(
            StaticProvider::new("azure", &[Capability::Chat], 90, "azure")
                .with_models(vec!["gpt-5.2".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["azure:gpt-5.*".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        assert_eq!(decision.selected.provider_id, "azure");
        assert_eq!(decision.selected.model, "gpt-5.2");
    }

    // ── Exclusion pattern tests ────────────────────────────────────

    #[test]
    fn exclusion_pattern_removes_matching_models() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai").with_models(vec![
                "gpt-5.1".into(),
                "gpt-5.2".into(),
                "gpt-5.4-mini".into(),
                "gpt-5.4".into(),
            ]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["gpt-5.*".into(), "!gpt-5.*-mini".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        // gpt-5.4-mini should be excluded; version-sorted: 5.4, 5.2, 5.1
        assert_eq!(decision.selected.model, "gpt-5.4");
        let fallback_models: Vec<&str> =
            decision.fallback_chain.iter().map(|s| s.model.as_str()).collect();
        assert!(!fallback_models.contains(&"gpt-5.4-mini"));
        assert!(fallback_models.contains(&"gpt-5.2"));
        assert!(fallback_models.contains(&"gpt-5.1"));
    }

    #[test]
    fn multiple_exclusion_patterns() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai").with_models(vec![
                "gpt-5.4".into(),
                "gpt-5.4-mini".into(),
                "gpt-5.4-nano".into(),
                "gpt-5.2".into(),
            ]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec![
                "gpt-5.*".into(),
                "!gpt-5.*-mini".into(),
                "!gpt-5.*-nano".into(),
            ]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        assert_eq!(decision.selected.model, "gpt-5.4");
        let all_models: Vec<&str> = std::iter::once(decision.selected.model.as_str())
            .chain(decision.fallback_chain.iter().map(|s| s.model.as_str()))
            .collect();
        assert_eq!(all_models, vec!["gpt-5.4", "gpt-5.2"]);
    }

    #[test]
    fn exclusion_with_provider_prefix() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.4".into(), "gpt-5.4-mini".into()]),
        );
        router.register_provider(
            StaticProvider::new("azure", &[Capability::Chat], 90, "azure")
                .with_models(vec!["gpt-5.4".into(), "gpt-5.4-mini".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec![
                "gpt-5.*".into(),
                "!openai:gpt-5.*-mini".into(), // only exclude mini from openai
            ]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        let all_models: Vec<(&str, &str)> =
            std::iter::once((&*decision.selected.provider_id, &*decision.selected.model))
                .chain(
                    decision
                        .fallback_chain
                        .iter()
                        .map(|s| (s.provider_id.as_str(), s.model.as_str())),
                )
                .collect();
        // openai:gpt-5.4-mini excluded, but azure:gpt-5.4-mini kept
        assert!(all_models.contains(&("azure", "gpt-5.4-mini")));
        assert!(!all_models.contains(&("openai", "gpt-5.4-mini")));
    }

    #[test]
    fn all_excluded_falls_through_to_priority() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.4-mini".into()]),
        );
        router.register_provider(
            StaticProvider::new("fallback", &[Capability::Chat], 50, "fallback")
                .with_models(vec!["default-model".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["gpt-5.*".into(), "!*-mini".into()]),
            ..basic_request()
        };

        // All preferred matches excluded → falls through to priority routing,
        // which picks the highest-priority provider regardless of exclusions.
        let decision = router.route(&request).expect("route");
        assert_eq!(decision.selected.provider_id, "openai");
        assert_eq!(decision.reason, "using provider priority order");
    }

    // ── Version-aware sorting tests ────────────────────────────────

    #[test]
    fn version_sorting_within_single_pattern() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("anthropic", &[Capability::Chat], 100, "anth").with_models(vec![
                "claude-sonnet-4.0".into(),
                "claude-sonnet-4.5".into(),
                "claude-sonnet-4.6".into(),
            ]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["claude-sonnet-*".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        // Should be sorted by version descending: 4.6, 4.5, 4.0
        assert_eq!(decision.selected.model, "claude-sonnet-4.6");
        let fallback: Vec<&str> =
            decision.fallback_chain.iter().map(|s| s.model.as_str()).collect();
        assert_eq!(fallback, vec!["claude-sonnet-4.5", "claude-sonnet-4.0"]);
    }

    #[test]
    fn version_sorting_preserves_cross_pattern_order() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.1".into(), "gpt-5.4".into()]),
        );
        router.register_provider(
            StaticProvider::new("anthropic", &[Capability::Chat], 90, "anth")
                .with_models(vec!["claude-sonnet-4.5".into(), "claude-sonnet-4.6".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec![
                "claude-sonnet-*".into(), // first pattern → Claude models first
                "gpt-5.*".into(),         // second pattern → GPT models after
            ]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        let all: Vec<&str> = std::iter::once(decision.selected.model.as_str())
            .chain(decision.fallback_chain.iter().map(|s| s.model.as_str()))
            .collect();
        // Claude first (version-sorted), then GPT (version-sorted)
        assert_eq!(all, vec!["claude-sonnet-4.6", "claude-sonnet-4.5", "gpt-5.4", "gpt-5.1"]);
    }

    #[test]
    fn version_sorting_unversioned_models_sort_last() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("provider", &[Capability::Chat], 100, "prov").with_models(vec![
                "custom-model".into(),
                "gpt-5.1".into(),
                "gpt-5.4".into(),
            ]),
        );

        let request =
            RoutingRequest { preferred_models: Some(vec!["*".into()]), ..basic_request() };

        let decision = router.route(&request).expect("route");
        let all: Vec<&str> = std::iter::once(decision.selected.model.as_str())
            .chain(decision.fallback_chain.iter().map(|s| s.model.as_str()))
            .collect();
        // Versioned models first (5.4, 5.1), unversioned last
        assert_eq!(all, vec!["gpt-5.4", "gpt-5.1", "custom-model"]);
    }

    // ── Combined exclusion + version sorting tests ─────────────────

    #[test]
    fn exclusion_and_version_sorting_combined() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai").with_models(vec![
                "gpt-5.1".into(),
                "gpt-5.2".into(),
                "gpt-5.4".into(),
                "gpt-5.4-mini".into(),
                "gpt-5.4-nano".into(),
            ]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["gpt-5.*".into(), "!*-mini".into(), "!*-nano".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        let all: Vec<&str> = std::iter::once(decision.selected.model.as_str())
            .chain(decision.fallback_chain.iter().map(|s| s.model.as_str()))
            .collect();
        assert_eq!(all, vec!["gpt-5.4", "gpt-5.2", "gpt-5.1"]);
    }

    #[test]
    fn exclusion_only_patterns_are_ignored() {
        // If there are ONLY exclusion patterns and no positive patterns,
        // fall through to priority routing.
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("openai", &[Capability::Chat], 100, "openai")
                .with_models(vec!["gpt-5.4".into()]),
        );

        let request = RoutingRequest {
            preferred_models: Some(vec!["!gpt-5.*-mini".into()]),
            ..basic_request()
        };

        let decision = router.route(&request).expect("route");
        // No positive patterns → falls through to priority routing
        assert_eq!(decision.selected.model, "gpt-5.4");
        assert_eq!(decision.reason, "using provider priority order");
    }

    #[test]
    fn falls_back_when_primary_provider_execution_fails() {
        let mut router = ModelRouter::new();
        router.register_provider(
            StaticProvider::new("primary", &[Capability::Chat], 100, "primary").failing(),
        );
        router.register_provider(StaticProvider::new(
            "fallback",
            &[Capability::Chat],
            50,
            "fallback",
        ));

        let response = router
            .complete(&CompletionRequest {
                prompt: "hello".to_string(),
                prompt_content_parts: vec![],
                messages: vec![],
                required_capabilities: [Capability::Chat].into_iter().collect(),
                preferred_models: None,
                tools: vec![],
            })
            .expect("completion");

        assert_eq!(response.provider_id, "fallback");
    }

    #[test]
    fn http_provider_executes_openai_compatible_completion() {
        let response_body = r#"{"choices":[{"message":{"content":"hello from http"}}]}"#;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock http provider");
        let address = listener.local_addr().expect("mock http provider address");
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept request");
            let mut buffer = [0_u8; 4096];
            let bytes_read = stream.read(&mut buffer).expect("read request");
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            assert!(request.contains(r#""role":"system""#));
            assert!(request.contains(r#""content":"You are helpful""#));
            assert!(request.contains(r#""role":"assistant""#));
            assert!(request.contains(r#""content":"Previous reply""#));
            assert!(request.contains(r#""role":"user""#));
            assert!(request.contains(r#""content":"hello""#));
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).expect("write response");
        });

        let provider = HttpProvider::new(
            ProviderDescriptor {
                id: "openrouter".to_string(),
                name: None,
                kind: ProviderKind::OpenAiCompatible,
                models: vec!["test-model".to_string()],
                model_capabilities: BTreeMap::from([(
                    "test-model".to_string(),
                    [Capability::Chat].into_iter().collect(),
                )]),
                priority: 100,
                available: true,
            },
            format!("http://{address}"),
            ProviderAuth::None,
        );

        let response = provider
            .complete(
                &CompletionRequest {
                    prompt: "hello".to_string(),
                    prompt_content_parts: vec![],
                    messages: vec![
                        CompletionMessage {
                            role: "system".to_string(),
                            content: "You are helpful".to_string(),
                            content_parts: vec![],
                        },
                        CompletionMessage {
                            role: "assistant".to_string(),
                            content: "Previous reply".to_string(),
                            content_parts: vec![],
                        },
                    ],
                    required_capabilities: [Capability::Chat].into_iter().collect(),
                    preferred_models: None,
                    tools: vec![],
                },
                &ModelSelection {
                    provider_id: "openrouter".to_string(),
                    model: "test-model".to_string(),
                },
            )
            .expect("http completion");

        assert_eq!(response.provider_id, "openrouter");
        assert_eq!(response.model, "test-model");
        assert_eq!(response.content, "hello from http");
        server.join().expect("join mock http server");
    }

    #[test]
    fn test_sanitize_openai_schema_fixes_array_type() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": ["string", "null"], "description": "A name" }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert_eq!(schema["properties"]["name"]["type"], "string");
    }

    #[test]
    fn test_sanitize_openai_schema_adds_missing_type() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "data": { "description": "Some data" }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert_eq!(schema["properties"]["data"]["type"], "string");
    }

    #[test]
    fn test_sanitize_openai_schema_adds_missing_items() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "tags": { "type": "array", "description": "Tags" }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert_eq!(schema["properties"]["tags"]["items"]["type"], "string");
    }

    #[test]
    fn test_sanitize_openai_schema_adds_properties_to_bare_object() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "meta": { "type": "object", "description": "Metadata" }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert!(schema["properties"]["meta"]["properties"].is_object());
    }

    #[test]
    fn test_sanitize_openai_schema_adds_additional_properties_false() {
        let mut schema = json!({
            "type": "object",
            "properties": {
                "name": { "type": "string" }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn test_sanitize_openai_schema_preserves_existing_additional_properties() {
        let mut schema = json!({
            "type": "object",
            "properties": {},
            "additionalProperties": { "type": "string" }
        });
        sanitize_openai_schema(&mut schema);
        // Should keep the existing value, not overwrite with false
        assert_eq!(schema["additionalProperties"]["type"], "string");
    }

    #[test]
    fn test_sanitize_openai_schema_strips_unsupported_keywords() {
        let mut schema = json!({
            "type": "object",
            "title": "MySchema",
            "$schema": "http://json-schema.org/draft-07/schema#",
            "default": {},
            "examples": [{}],
            "$id": "some-id",
            "$comment": "a comment",
            "properties": {
                "name": { "type": "string", "default": "foo", "examples": ["bar"] }
            }
        });
        sanitize_openai_schema(&mut schema);
        assert!(schema.get("title").is_none());
        assert!(schema.get("$schema").is_none());
        assert!(schema.get("default").is_none());
        assert!(schema.get("examples").is_none());
        assert!(schema.get("$id").is_none());
        assert!(schema.get("$comment").is_none());
        // Stripped from nested property too
        assert!(schema["properties"]["name"].get("default").is_none());
        assert!(schema["properties"]["name"].get("examples").is_none());
    }

    /// Helper: recursively validate that a JSON Schema object is compliant
    /// with the strict subset accepted by OpenAI / GitHub Copilot.
    fn assert_openai_compliant(val: &serde_json::Value, path: &str) {
        let obj = match val.as_object() {
            Some(o) => o,
            None => return,
        };

        // Unsupported keywords must be absent
        for key in &["default", "examples", "$schema", "$id", "title", "$comment"] {
            assert!(!obj.contains_key(*key), "{path}: contains unsupported keyword '{key}'");
        }

        let type_val = obj.get("type");
        if let Some(tv) = type_val {
            assert!(tv.is_string(), "{path}: 'type' must be a string, got {tv}");
        }

        let current_type = type_val.and_then(|v| v.as_str());

        if current_type == Some("object") {
            assert!(obj.contains_key("properties"), "{path}: object must have 'properties'");
            assert!(
                obj.contains_key("additionalProperties"),
                "{path}: object must have 'additionalProperties'"
            );
            if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
                for (key, prop_schema) in props {
                    let child_path = format!("{path}.properties.{key}");
                    if let Some(po) = prop_schema.as_object() {
                        assert!(po.contains_key("type"), "{child_path}: property must have 'type'");
                    }
                    assert_openai_compliant(prop_schema, &child_path);
                }
            }
        }

        if current_type == Some("array") {
            assert!(obj.contains_key("items"), "{path}: array must have 'items'");
            if let Some(items) = obj.get("items") {
                assert_openai_compliant(items, &format!("{path}.items"));
            }
        }

        if let Some(addl) = obj.get("additionalProperties") {
            if addl.is_object() {
                assert_openai_compliant(addl, &format!("{path}.additionalProperties"));
            }
        }
    }

    /// Regression test: run ALL 13 registered tool schemas through
    /// `format_tools_openai()` and verify every resulting definition is
    /// OpenAI-compliant (no array types, all objects have properties &
    /// additionalProperties, etc.).
    #[test]
    fn test_all_tool_schemas_openai_compliant() {
        use hive_classification::ChannelClass;
        use hive_contracts::{ToolAnnotations, ToolApproval};

        // Replicate the exact input_schema for each of the 13 registered tools
        let tool_schemas: Vec<(&str, &str, serde_json::Value)> = vec![
            (
                "core.echo",
                "Echo a value",
                json!({
                    "type": "object",
                    "properties": {
                        "value": { "description": "Value to echo back.", "type": "string" }
                    },
                    "required": ["value"]
                }),
            ),
            (
                "filesystem.read",
                "Read file contents",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path to the file." }
                    },
                    "required": ["path"]
                }),
            ),
            (
                "filesystem.list",
                "List directory contents",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path to the directory." }
                    },
                    "required": ["path"]
                }),
            ),
            (
                "filesystem.exists",
                "Check if file/dir exists",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path to check." }
                    },
                    "required": ["path"]
                }),
            ),
            (
                "filesystem.write",
                "Write file contents",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative path to write." },
                        "content": { "type": "string", "description": "Text content to write." },
                        "overwrite": { "type": "boolean", "description": "Overwrite if the file exists." }
                    },
                    "required": ["path", "content"]
                }),
            ),
            (
                "filesystem.search",
                "Search file contents",
                json!({
                    "type": "object",
                    "properties": {
                        "path": { "type": "string", "description": "Relative directory or file path." },
                        "query": { "type": "string", "description": "Search term." },
                        "limit": { "type": "number", "description": "Maximum matches to return." },
                        "caseSensitive": { "type": "boolean", "description": "Case-sensitive search." }
                    },
                    "required": ["path", "query"]
                }),
            ),
            (
                "filesystem.glob",
                "Glob file patterns",
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Glob pattern relative to the workspace." },
                        "limit": { "type": "number", "description": "Maximum number of matches to return." }
                    },
                    "required": ["pattern"]
                }),
            ),
            (
                "shell.execute",
                "Execute shell command",
                json!({
                    "type": "object",
                    "properties": {
                        "command": { "type": "string", "description": "Shell command to execute." },
                        "working_dir": { "type": "string", "description": "Optional working directory." },
                        "timeout_secs": { "type": "number", "description": "Timeout in seconds (default 300)." }
                    },
                    "required": ["command"]
                }),
            ),
            (
                "http.request",
                "Make HTTP request",
                json!({
                    "type": "object",
                    "properties": {
                        "method": { "type": "string", "description": "HTTP method." },
                        "url": { "type": "string", "description": "URL to request." },
                        "headers": { "type": "object", "description": "Optional HTTP headers.", "additionalProperties": { "type": "string" } },
                        "body": { "type": "string", "description": "Optional request body." }
                    },
                    "required": ["method", "url"]
                }),
            ),
            (
                "knowledge.query",
                "Query knowledge base",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Search query." },
                        "limit": { "type": "number", "description": "Maximum results to return." }
                    },
                    "required": ["query"]
                }),
            ),
            (
                "math.calculate",
                "Evaluate math expression",
                json!({
                    "type": "object",
                    "properties": {
                        "expression": { "type": "string", "description": "Math expression to evaluate." }
                    },
                    "required": ["expression"]
                }),
            ),
            (
                "datetime.now",
                "Get current date/time",
                json!({
                    "type": "object",
                    "properties": {
                        "format": { "type": "string", "description": "Output format (default: ISO-8601)." }
                    }
                }),
            ),
            (
                "json.transform",
                "Transform JSON data",
                json!({
                    "type": "object",
                    "properties": {
                        "data": { "type": "object", "description": "JSON data to query." },
                        "path": { "type": "string", "description": "Dot-notation path." }
                    },
                    "required": ["data", "path"]
                }),
            ),
        ];

        let tools: Vec<ToolDefinition> = tool_schemas
            .into_iter()
            .map(|(id, desc, schema)| ToolDefinition {
                id: id.to_string(),
                name: id.to_string(),
                description: desc.to_string(),
                input_schema: schema,
                output_schema: None,
                channel_class: ChannelClass::Public,
                side_effects: false,
                approval: ToolApproval::Auto,
                annotations: ToolAnnotations {
                    title: id.to_string(),
                    read_only_hint: None,
                    destructive_hint: None,
                    idempotent_hint: None,
                    open_world_hint: None,
                },
            })
            .collect();

        let formatted =
            format_tools_openai(&tools).expect("should produce Some for non-empty tools");

        assert_eq!(formatted.len(), 13, "expected 13 formatted tools");

        for tool_val in &formatted {
            let tool = tool_val.as_object().expect("tool should be an object");
            assert_eq!(tool.get("type").unwrap(), "function");
            let func = tool
                .get("function")
                .and_then(|f| f.as_object())
                .expect("tool.function should be an object");

            let name = func.get("name").and_then(|n| n.as_str()).expect("name");
            let desc = func.get("description").and_then(|d| d.as_str()).expect("description");
            let params = func.get("parameters").expect("parameters");

            // Name must match OpenAI pattern: ^[a-zA-Z0-9_-]{1,64}$
            assert!(
                name.len() <= 64
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
                "tool name '{name}' does not match OpenAI pattern"
            );
            // No dots in sanitized names
            assert!(!name.contains('.'), "tool name '{name}' still contains dots");
            // Description must be non-empty
            assert!(!desc.is_empty(), "tool '{name}' has empty description");

            // Deep-validate the parameters schema
            assert_openai_compliant(params, &format!("tool[{name}].parameters"));
        }
    }

    // ── Secret store integration tests ────────────────────────────
    // These tests exercise the secret_store abstraction layer.

    #[test]
    fn secret_store_save_and_load() {
        hive_core::secret_store::save("test:model-save-load", "my-secret-123");
        let loaded = hive_core::secret_store::load("test:model-save-load");
        assert_eq!(loaded, Some("my-secret-123".to_string()));
        hive_core::secret_store::delete("test:model-save-load");
    }

    #[test]
    fn secret_store_load_missing_returns_none() {
        hive_core::secret_store::delete("test:model-missing-key");
        let result = hive_core::secret_store::load("test:model-missing-key");
        assert!(result.is_none());
    }

    #[test]
    fn secret_store_overwrite_existing() {
        hive_core::secret_store::save("test:model-overwrite", "first-value");
        hive_core::secret_store::save("test:model-overwrite", "second-value");
        let loaded = hive_core::secret_store::load("test:model-overwrite");
        assert_eq!(loaded, Some("second-value".to_string()));
        hive_core::secret_store::delete("test:model-overwrite");
    }

    #[test]
    fn read_keyring_helper_succeeds() {
        hive_core::secret_store::save("test:model-read-helper", "helper-test-value");
        let result = read_keyring("test:model-read-helper");
        assert!(result.is_ok(), "read_keyring should succeed: {result:?}");
        assert_eq!(result.unwrap(), "helper-test-value");
        hive_core::secret_store::delete("test:model-read-helper");
    }

    #[test]
    fn read_keyring_helper_missing() {
        hive_core::secret_store::delete("test:model-read-helper-missing");
        let result = read_keyring("test:model-read-helper-missing");
        assert!(result.is_err(), "read_keyring should fail for missing key");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("no secret found"),
            "error should mention 'no secret found', got: {err_msg}"
        );
    }

    #[test]
    fn read_keyring_provider_key_pattern() {
        let provider_id = "test-provider-123";
        let key = format!("provider:{provider_id}:api-key");
        hive_core::secret_store::save(&key, "sk-ant-test-key-abc123");
        let loaded = read_keyring(&key).expect("read via helper");
        assert_eq!(loaded, "sk-ant-test-key-abc123");
        hive_core::secret_store::delete(&key);
    }

    // ── Error classification tests ──────────────────────────────────

    #[test]
    fn classify_rate_limit_error() {
        assert_eq!(
            classify_error_message("provider openai returned 429: rate limited"),
            ProviderErrorKind::RateLimited,
        );
    }

    #[test]
    fn classify_insufficient_credits() {
        assert_eq!(
            classify_error_message("provider openai returned 402: insufficient credits"),
            ProviderErrorKind::InsufficientCredits,
        );
    }

    #[test]
    fn classify_anthropic_credit_balance_400() {
        let msg = r#"provider 278e3a4d returned 400 Bad Request: {"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits."}}"#;
        assert_eq!(classify_error_message(msg), ProviderErrorKind::InsufficientCredits,);
    }

    #[test]
    fn classify_auth_error_401() {
        assert_eq!(
            classify_error_message("provider openai returned 401: unauthorized"),
            ProviderErrorKind::AuthError,
        );
    }

    #[test]
    fn classify_auth_error_403() {
        assert_eq!(
            classify_error_message("provider openai returned 403: forbidden"),
            ProviderErrorKind::AuthError,
        );
    }

    #[test]
    fn classify_server_error_502() {
        assert_eq!(
            classify_error_message("provider openai returned 502: bad gateway"),
            ProviderErrorKind::ServerError,
        );
    }

    #[test]
    fn classify_server_error_503() {
        assert_eq!(
            classify_error_message("provider openai returned 503: service unavailable"),
            ProviderErrorKind::ServerError,
        );
    }

    #[test]
    fn classify_connection_failure_as_retryable() {
        assert_eq!(
            classify_error_message("failed to contact provider openai at https://api.openai.com"),
            ProviderErrorKind::ServerError,
        );
    }

    #[test]
    fn classify_unknown_error() {
        assert_eq!(
            classify_error_message("something completely different"),
            ProviderErrorKind::Other,
        );
    }

    #[test]
    fn classify_400_as_other() {
        assert_eq!(
            classify_error_message("provider openai returned 400: bad request"),
            ProviderErrorKind::Other,
        );
    }

    #[test]
    fn retryable_errors() {
        assert!(ProviderErrorKind::RateLimited.is_retryable());
        assert!(ProviderErrorKind::ServerError.is_retryable());
        assert!(!ProviderErrorKind::AuthError.is_retryable());
        assert!(!ProviderErrorKind::InsufficientCredits.is_retryable());
        assert!(!ProviderErrorKind::Other.is_retryable());
    }

    // -- Retry tests ---------------------------------------------------------

    use std::sync::atomic::AtomicU32;

    /// A provider that fails with a configurable error message for a number of
    /// calls, then succeeds.
    struct TransientFailProvider {
        descriptor: ProviderDescriptor,
        /// Error message to produce on failure.
        error_msg: String,
        /// Number of times to fail before succeeding.
        fail_count: u32,
        /// How many times `complete` has been called.
        call_count: Arc<AtomicU32>,
    }

    impl TransientFailProvider {
        fn new(id: &str, error_msg: &str, fail_count: u32) -> Self {
            Self {
                descriptor: ProviderDescriptor {
                    id: id.to_string(),
                    name: None,
                    kind: ProviderKind::Mock,
                    model_capabilities: BTreeMap::from([(
                        "default".to_string(),
                        [Capability::Chat].into_iter().collect(),
                    )]),
                    models: vec!["default".to_string()],
                    priority: 100,
                    available: true,
                },
                error_msg: error_msg.to_string(),
                fail_count,
                call_count: Arc::new(AtomicU32::new(0)),
            }
        }

        fn call_count(&self) -> u32 {
            self.call_count.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn call_count_arc(&self) -> Arc<AtomicU32> {
            Arc::clone(&self.call_count)
        }
    }

    impl ModelProvider for TransientFailProvider {
        fn descriptor(&self) -> &ProviderDescriptor {
            &self.descriptor
        }

        fn complete(
            &self,
            _request: &CompletionRequest,
            selection: &ModelSelection,
        ) -> Result<CompletionResponse> {
            let n = self.call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.fail_count {
                bail!("{}", self.error_msg);
            }
            Ok(CompletionResponse {
                provider_id: self.descriptor.id.clone(),
                model: selection.model.clone(),
                content: "ok".to_string(),
                tool_calls: vec![],
            })
        }
    }

    /// Zero-backoff policy for fast tests.
    fn test_retry_policy() -> RetryPolicy {
        RetryPolicy {
            max_attempts: RETRY_MAX_ATTEMPTS,
            initial_backoff_ms: 0,
            multiplier: 1.0,
            max_backoff_ms: 0,
        }
    }

    fn test_request() -> CompletionRequest {
        CompletionRequest {
            prompt: "hello".into(),
            prompt_content_parts: vec![],
            messages: vec![],
            required_capabilities: [Capability::Chat].into_iter().collect(),
            preferred_models: None,
            tools: vec![],
        }
    }

    #[test]
    fn retry_on_transient_429_succeeds_after_retries() {
        // Provider fails twice with 429, then succeeds.
        let provider =
            TransientFailProvider::new("test", "provider test returned 429: rate limited", 2);
        let call_counter = provider.call_count_arc();

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();
        let result =
            router.complete_with_retry(&test_request(), &decision, None, &test_retry_policy());
        assert!(result.is_ok(), "should succeed after retries: {result:?}");
        // 2 failures + 1 success = 3 calls
        assert_eq!(call_counter.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn retry_on_transient_500_succeeds_after_retries() {
        let provider = TransientFailProvider::new(
            "test",
            "provider test returned 500: internal server error",
            1,
        );
        let call_counter = provider.call_count_arc();

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();
        let result =
            router.complete_with_retry(&test_request(), &decision, None, &test_retry_policy());
        assert!(result.is_ok());
        assert_eq!(call_counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn no_retry_on_auth_error() {
        // Provider always fails with 401 — should NOT be retried.
        let provider = TransientFailProvider::new(
            "test",
            "provider test returned 401: unauthorized",
            100, // always fail
        );
        let call_counter = provider.call_count_arc();

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();
        let result =
            router.complete_with_retry(&test_request(), &decision, None, &test_retry_policy());
        assert!(result.is_err());
        // Auth error: only 1 call, no retries
        assert_eq!(call_counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn no_retry_on_credit_error() {
        let provider = TransientFailProvider::new(
            "test",
            "provider test returned 402: insufficient credits",
            100,
        );
        let call_counter = provider.call_count_arc();

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();
        let result =
            router.complete_with_retry(&test_request(), &decision, None, &test_retry_policy());
        assert!(result.is_err());
        assert_eq!(call_counter.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_exhaustion_returns_structured_error() {
        // Provider always fails with 503 — exhausts all retries.
        let provider = TransientFailProvider::new(
            "test",
            "provider test returned 503: service unavailable",
            100, // never succeed
        );
        let call_counter = provider.call_count_arc();

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();
        let result =
            router.complete_with_retry(&test_request(), &decision, None, &test_retry_policy());
        assert!(result.is_err());
        let err = result.unwrap_err();

        // Should have tried 1 initial + RETRY_MAX_ATTEMPTS retries
        assert_eq!(call_counter.load(std::sync::atomic::Ordering::SeqCst), RETRY_MAX_ATTEMPTS + 1);

        // Error should carry structured fields.
        if let ModelRouterError::ProviderExecutionFailed { error_kind, http_status, .. } = &err {
            assert_eq!(*error_kind, Some(ProviderErrorKind::ServerError));
            assert_eq!(*http_status, Some(503));
        } else {
            panic!("expected ProviderExecutionFailed, got: {err:?}");
        }
    }

    #[test]
    fn retry_callback_is_invoked() {
        let provider = TransientFailProvider::new(
            "test",
            "provider test returned 502: bad gateway",
            2, // fail twice, then succeed
        );

        let mut router = ModelRouter::new();
        router.register_provider(provider);

        let decision = router.route(&basic_request()).unwrap();

        let retry_infos: Arc<std::sync::Mutex<Vec<RetryInfo>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let infos_clone = Arc::clone(&retry_infos);
        let cb = move |info: &RetryInfo| {
            infos_clone.lock().unwrap().push(info.clone());
        };

        let result =
            router.complete_with_retry(&test_request(), &decision, Some(&cb), &test_retry_policy());
        assert!(result.is_ok());

        let infos = retry_infos.lock().unwrap();
        assert_eq!(infos.len(), 2, "should have 2 retry callbacks");
        assert_eq!(infos[0].attempt, 1);
        assert_eq!(infos[1].attempt, 2);
        assert_eq!(infos[0].error_kind, ProviderErrorKind::ServerError);
        assert!(infos[0].http_status == Some(502));
    }

    #[test]
    fn classify_error_message_http_500() {
        assert_eq!(
            classify_error_message("provider test returned 500: internal server error"),
            ProviderErrorKind::ServerError,
        );
    }

    #[test]
    fn classify_error_message_connection_failure() {
        assert_eq!(
            classify_error_message("failed to contact provider test"),
            ProviderErrorKind::ServerError,
        );
    }

    #[test]
    fn provider_error_kind_serde_roundtrip() {
        let kind = ProviderErrorKind::RateLimited;
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, r#""rate_limited""#);
        let parsed: ProviderErrorKind = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, kind);
    }

    #[test]
    fn retry_backoff_values_are_reasonable() {
        // Attempt 0 → ~5s (3.75s – 6.25s with jitter)
        let b0 = retry_backoff_ms(0);
        assert!(b0 >= 3_500 && b0 <= 6_500, "attempt 0 backoff was {b0}ms");

        // Attempt 1 → ~10s
        let b1 = retry_backoff_ms(1);
        assert!(b1 >= 7_000 && b1 <= 13_000, "attempt 1 backoff was {b1}ms");

        // Attempt 3 → capped at ~40s
        let b3 = retry_backoff_ms(3);
        assert!(b3 >= 28_000 && b3 <= 52_000, "attempt 3 backoff was {b3}ms");

        // Attempt 4 → still capped at ~40s
        let b4 = retry_backoff_ms(4);
        assert!(b4 >= 28_000 && b4 <= 52_000, "attempt 4 backoff was {b4}ms");
    }

    #[test]
    fn extract_http_status_from_anyhow_error() {
        let err = anyhow!("provider test returned 429: too many requests");
        assert_eq!(extract_http_status(&err), Some(429));
    }

    #[test]
    fn extract_http_status_from_provider_error() {
        let pe = ProviderError {
            provider_id: "test".into(),
            model: Some("gpt-4".into()),
            kind: ProviderErrorKind::RateLimited,
            http_status: Some(429),
            message: "rate limited".into(),
        };
        let err: anyhow::Error = pe.into();
        assert_eq!(extract_http_status(&err), Some(429));
        assert_eq!(classify_provider_error(&err), ProviderErrorKind::RateLimited);
    }
}
