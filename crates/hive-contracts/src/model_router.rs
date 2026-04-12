use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Capability {
    Chat,
    Code,
    Vision,
    Embedding,
    ToolUse,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderKind {
    OpenAiCompatible,
    Anthropic,
    #[serde(rename = "microsoft-foundry", alias = "azure-foundry")]
    MicrosoftFoundry,
    #[serde(rename = "github-copilot")]
    GitHubCopilot,
    OllamaLocal,
    LocalRuntime,
    Mock,
}

static EMPTY_CAPABILITIES: LazyLock<BTreeSet<Capability>> = LazyLock::new(BTreeSet::new);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDescriptor {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub kind: ProviderKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub model_capabilities: BTreeMap<String, BTreeSet<Capability>>,
    pub models: Vec<String>,
    pub priority: i32,
    pub available: bool,
}

impl ProviderDescriptor {
    /// Returns the capabilities for a specific model.
    pub fn capabilities_for_model(&self, model: &str) -> &BTreeSet<Capability> {
        self.model_capabilities.get(model).unwrap_or(&EMPTY_CAPABILITIES)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelRouterSnapshot {
    pub providers: Vec<ProviderDescriptor>,
}
