//! Azure Default Credential helpers for Microsoft Foundry.
//!
//! `auth: azure-default` uses a credential chain:
//! 1. Managed Identity — only when Azure IMDS or hosting env vars are present
//! 2. Azure CLI (`az login`)
//! 3. Azure Developer CLI (`azd auth login`)
//!
//! Managed Identity is skipped on laptops. The Azure SDK otherwise retries IMDS
//! (`169.254.169.254`) for minutes, which exceeds the desktop Fetch Models timeout.

use anyhow::{anyhow, Context, Result};
use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use azure_core::http::{ClientOptions, RetryOptions};
use azure_identity::{
    AzureCliCredential, AzureDeveloperCliCredential, ManagedIdentityCredential,
    ManagedIdentityCredentialOptions,
};
use std::env;
use std::net::{SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Scope required for Azure AI / Cognitive Services inference endpoints.
pub(crate) const AZURE_COGNITIVE_SCOPE: &str = "https://cognitiveservices.azure.com/.default";

/// Scope required for Azure AI Foundry Projects API (deployment listing, etc.).
pub(crate) const AZURE_AI_SCOPE: &str = "https://ai.azure.com/.default";

const SKIP_MI_ENV: &str = "HIVEMIND_AZURE_SKIP_MANAGED_IDENTITY";
const IMDS_PROBE_TIMEOUT: Duration = Duration::from_millis(400);
const MANAGED_IDENTITY_TOKEN_TIMEOUT: Duration = Duration::from_secs(3);

fn imds_addr() -> SocketAddr {
    SocketAddr::from(([169, 254, 169, 254], 80))
}

/// Cached Azure AD bearer token with expiry.
#[derive(Clone, Debug)]
struct AzureTokenCache {
    token: String,
    /// Unix timestamp (seconds) when this token expires.
    expires_at: u64,
}

/// Thread-safe cache for Azure AD tokens, keyed by scope.
static AZURE_TOKEN_CACHE: std::sync::LazyLock<
    parking_lot::Mutex<std::collections::HashMap<String, AzureTokenCache>>,
> = std::sync::LazyLock::new(|| parking_lot::Mutex::new(std::collections::HashMap::new()));

/// Lazily-initialised Azure credential chain.
static AZURE_CREDENTIAL: std::sync::LazyLock<Result<Arc<dyn TokenCredential>>> =
    std::sync::LazyLock::new(build_credential_chain);

fn env_flag_enabled(name: &str) -> bool {
    match env::var(name) {
        Ok(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        Err(_) => false,
    }
}

fn azure_hosted_env_signals() -> bool {
    azure_hosted_env_signals_from(|k| env::var_os(k).is_some())
}

fn azure_hosted_env_signals_from(has_var: impl Fn(&str) -> bool) -> bool {
    ["IDENTITY_ENDPOINT", "MSI_ENDPOINT", "AZURE_FEDERATED_TOKEN_FILE"].iter().any(|k| has_var(k))
}

/// True when TCP port 80 on the Azure IMDS link-local address accepts a connection.
fn imds_endpoint_reachable(timeout: Duration) -> bool {
    TcpStream::connect_timeout(&imds_addr(), timeout).is_ok()
}

fn should_include_managed_identity() -> bool {
    should_include_managed_identity_with(
        env_flag_enabled(SKIP_MI_ENV),
        azure_hosted_env_signals(),
        || imds_endpoint_reachable(IMDS_PROBE_TIMEOUT),
    )
}

fn should_include_managed_identity_with(
    skip: bool,
    hosted_env: bool,
    imds_reachable: impl Fn() -> bool,
) -> bool {
    if skip {
        tracing::info!("skipping Azure Managed Identity ({SKIP_MI_ENV} is set)");
        return false;
    }
    if hosted_env {
        return true;
    }
    let reachable = imds_reachable();
    if !reachable {
        tracing::info!(
            "skipping Azure Managed Identity (IMDS at 169.254.169.254 not reachable in {:?}); falling back to Azure CLI / Developer CLI",
            IMDS_PROBE_TIMEOUT
        );
    }
    reachable
}

fn build_credential_chain() -> Result<Arc<dyn TokenCredential>> {
    let mut sources: Vec<AzureCredentialSource> = Vec::new();

    if should_include_managed_identity() {
        let options = ManagedIdentityCredentialOptions {
            client_options: ClientOptions { retry: RetryOptions::none(), ..Default::default() },
            ..Default::default()
        };
        match ManagedIdentityCredential::new(Some(options)) {
            Ok(c) => {
                tracing::info!("Azure credential chain: Managed Identity enabled");
                sources.push(AzureCredentialSource {
                    name: "managed-identity",
                    credential: c as Arc<dyn TokenCredential>,
                    timeout: Some(MANAGED_IDENTITY_TOKEN_TIMEOUT),
                });
            }
            Err(e) => {
                tracing::debug!("Managed Identity credential not constructed: {e}");
            }
        }
    }

    if let Ok(c) = AzureCliCredential::new(None) {
        tracing::info!("Azure credential chain: Azure CLI enabled");
        sources.push(AzureCredentialSource {
            name: "azure-cli",
            credential: c as Arc<dyn TokenCredential>,
            timeout: None,
        });
    }
    if let Ok(c) = AzureDeveloperCliCredential::new(None) {
        tracing::info!("Azure credential chain: Azure Developer CLI enabled");
        sources.push(AzureCredentialSource {
            name: "azure-developer-cli",
            credential: c as Arc<dyn TokenCredential>,
            timeout: None,
        });
    }

    if sources.is_empty() {
        return Err(anyhow!(
            "no Azure credential sources available. On a local machine, install Azure CLI and run `az login`. On Azure compute, assign a managed identity. Alternatively, configure the Foundry provider with an API key."
        ));
    }

    let names: Vec<&str> = sources.iter().map(|s| s.name).collect();
    tracing::info!(sources = ?names, "Azure credential chain initialised");
    Ok(Arc::new(AzureCredentialChain { sources }) as Arc<dyn TokenCredential>)
}

struct AzureCredentialSource {
    name: &'static str,
    credential: Arc<dyn TokenCredential>,
    timeout: Option<Duration>,
}

/// Tries each credential until one succeeds.
struct AzureCredentialChain {
    sources: Vec<AzureCredentialSource>,
}

impl std::fmt::Debug for AzureCredentialChain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.sources.iter().map(|s| s.name).collect();
        f.debug_struct("AzureCredentialChain").field("sources", &names).finish()
    }
}

#[async_trait::async_trait]
impl TokenCredential for AzureCredentialChain {
    async fn get_token(
        &self,
        scopes: &[&str],
        options: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        let mut last_error = None;
        for source in &self.sources {
            let result = if let Some(timeout) = source.timeout {
                match tokio::time::timeout(
                    timeout,
                    source.credential.get_token(scopes, options.clone()),
                )
                .await
                {
                    Ok(inner) => inner,
                    Err(_) => {
                        tracing::warn!(
                            source = source.name,
                            timeout_ms = timeout.as_millis() as u64,
                            "Azure credential timed out, trying next source"
                        );
                        last_error = Some(azure_core::Error::with_message(
                            azure_core::error::ErrorKind::Credential,
                            format!("{} timed out after {:?}", source.name, timeout),
                        ));
                        continue;
                    }
                }
            } else {
                source.credential.get_token(scopes, options.clone()).await
            };

            match result {
                Ok(token) => {
                    tracing::info!(source = source.name, "acquired Azure token");
                    return Ok(token);
                }
                Err(e) => {
                    tracing::warn!(source = source.name, error = %e, "Azure credential failed, trying next source");
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| {
            azure_core::Error::with_message(
                azure_core::error::ErrorKind::Credential,
                "no credential sources available".to_string(),
            )
        }))
    }
}

/// Acquire an Azure AD bearer token for the given scope, using a cached value
/// when possible.
///
/// Tokens are refreshed when within 5 minutes of expiry.
pub(crate) fn get_azure_token_blocking(scope: &str) -> Result<String> {
    let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

    {
        let cache = AZURE_TOKEN_CACHE.lock();
        if let Some(cached) = cache.get(scope) {
            if cached.expires_at > now + 300 {
                return Ok(cached.token.clone());
            }
        }
    }

    let access_token = match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            tokio::task::block_in_place(|| handle.block_on(acquire_azure_token_async(scope)))?
        }
        Err(_) => {
            let rt = tokio::runtime::Runtime::new()
                .context("failed to create tokio runtime for Azure credential")?;
            rt.block_on(acquire_azure_token_async(scope))?
        }
    };

    let token = access_token.token.clone();
    AZURE_TOKEN_CACHE.lock().insert(scope.to_string(), access_token);
    Ok(token)
}

#[allow(dead_code)]
pub(crate) async fn get_azure_token_async(scope: &str) -> Result<String> {
    let now = SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();

    {
        let cache = AZURE_TOKEN_CACHE.lock();
        if let Some(cached) = cache.get(scope) {
            if cached.expires_at > now + 300 {
                return Ok(cached.token.clone());
            }
        }
    }

    let access_token = acquire_azure_token_async(scope).await?;
    let token = access_token.token.clone();
    AZURE_TOKEN_CACHE.lock().insert(scope.to_string(), access_token);
    Ok(token)
}

async fn acquire_azure_token_async(scope: &str) -> Result<AzureTokenCache> {
    let credential = AZURE_CREDENTIAL.as_ref().map_err(|e| anyhow!("{e}")).context(
        "Azure credential chain not available. Sign in with `az login` or `azd auth login`, or use a Foundry API key.",
    )?;

    let response = credential.get_token(&[scope], None).await.map_err(|e| {
        anyhow!(
            "Azure token acquisition failed: {e}. Sign in with `az login` (or `azd auth login`), or configure the Foundry provider with an API key. Managed identity is used only on Azure-hosted compute."
        )
    })?;

    let token = response.token.secret().to_string();
    let expires_at = response.expires_on.unix_timestamp() as u64;

    Ok(AzureTokenCache { token, expires_at })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_flag_parses_truthy_values() {
        assert!(matches!("1".trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
        assert!(matches!("TRUE".trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
        assert!(!matches!("0".trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"));
    }

    #[test]
    fn hosted_env_signals_identity_endpoint() {
        assert!(azure_hosted_env_signals_from(|k| k == "IDENTITY_ENDPOINT"));
        assert!(azure_hosted_env_signals_from(|k| k == "MSI_ENDPOINT"));
        assert!(azure_hosted_env_signals_from(|k| k == "AZURE_FEDERATED_TOKEN_FILE"));
        assert!(!azure_hosted_env_signals_from(|_| false));
        assert!(!azure_hosted_env_signals_from(|k| k == "PATH"));
    }

    #[test]
    fn skip_flag_disables_managed_identity_even_on_azure() {
        assert!(!should_include_managed_identity_with(true, true, || true));
    }

    #[test]
    fn hosted_env_enables_managed_identity_without_imds() {
        assert!(should_include_managed_identity_with(false, true, || false));
    }

    #[test]
    fn laptop_without_imds_skips_managed_identity() {
        assert!(!should_include_managed_identity_with(false, false, || false));
    }

    #[test]
    fn azure_vm_imds_enables_managed_identity() {
        assert!(should_include_managed_identity_with(false, false, || true));
    }

    #[test]
    fn imds_probe_fails_fast_when_unavailable() {
        let start = std::time::Instant::now();
        let _reachable = imds_endpoint_reachable(Duration::from_millis(400));
        let elapsed = start.elapsed();
        assert!(
            elapsed < Duration::from_secs(2),
            "IMDS probe hung for {elapsed:?}; expected connect_timeout to fail fast"
        );
    }
}
