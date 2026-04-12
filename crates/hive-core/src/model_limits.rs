//! Static registry of known model metadata (context windows, output limits,
//! and capabilities).
//!
//! Provides [`ModelMetadataRegistry`] which loads model metadata from an
//! embedded YAML file and exposes a longest-prefix-match lookup for any
//! model name string.
//!
//! Legacy aliases [`ModelLimits`] and [`ModelLimitsRegistry`] are provided
//! for backward compatibility.

use std::collections::BTreeSet;

use hive_contracts::CapabilityConfig;
use serde::Deserialize;
use tracing::debug;

/// Metadata for a single model: token limits and known capabilities.
#[derive(Debug, Clone)]
pub struct ModelMetadata {
    /// Total context window in tokens (input + output share this budget).
    pub context_window: u32,
    /// Maximum tokens the model will generate in one response.
    pub max_output_tokens: u32,
    /// Known capabilities of this model (empty = unknown; caller decides defaults).
    pub capabilities: BTreeSet<CapabilityConfig>,
}

impl ModelMetadata {
    /// Maximum *input* tokens: context window minus output budget.
    pub fn max_input_tokens(&self) -> u32 {
        self.context_window.saturating_sub(self.max_output_tokens)
    }
}

/// Legacy alias — use [`ModelMetadata`] instead.
pub type ModelLimits = ModelMetadata;

// ── YAML schema ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct MetadataFile {
    default_context_window: u32,
    models: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    pattern: String,
    context_window: u32,
    #[serde(default)]
    max_output_tokens: Option<u32>,
    #[serde(default)]
    capabilities: Vec<CapabilityConfig>,
}

// ── Registry ────────────────────────────────────────────────────────

/// Sorted list of (pattern, metadata) pairs. Sorted by descending pattern
/// length so that the first matching prefix is the most specific.
pub struct ModelMetadataRegistry {
    entries: Vec<(String, ModelMetadata)>,
    default_context_window: u32,
}

/// Legacy alias — use [`ModelMetadataRegistry`] instead.
pub type ModelLimitsRegistry = ModelMetadataRegistry;

static EMBEDDED_YAML: &str = include_str!("../model-metadata.yaml");

impl ModelMetadataRegistry {
    /// Load from the embedded `model-metadata.yaml`.
    pub fn load() -> Self {
        Self::from_yaml(EMBEDDED_YAML).expect("embedded model-metadata.yaml is invalid")
    }

    /// Parse from a YAML string (also useful for tests).
    pub fn from_yaml(yaml: &str) -> Result<Self, serde_yaml::Error> {
        let file: MetadataFile = serde_yaml::from_str(yaml)?;
        let mut entries: Vec<(String, ModelMetadata)> = file
            .models
            .into_iter()
            .map(|entry| {
                let metadata = ModelMetadata {
                    context_window: entry.context_window,
                    max_output_tokens: entry.max_output_tokens.unwrap_or(4096),
                    capabilities: entry.capabilities.into_iter().collect(),
                };
                (entry.pattern.to_lowercase(), metadata)
            })
            .collect();
        // Sort by descending pattern length so longest prefix matches first.
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Ok(Self { entries, default_context_window: file.default_context_window })
    }

    /// Look up metadata for a model name. Returns the entry with the longest
    /// matching prefix, or a conservative default.
    pub fn lookup(&self, model_name: &str) -> ModelMetadata {
        let name = model_name.to_lowercase();
        for (pattern, metadata) in &self.entries {
            if name.starts_with(pattern.as_str()) {
                debug!(
                    model = model_name,
                    matched_pattern = pattern.as_str(),
                    context_window = metadata.context_window,
                    max_output_tokens = metadata.max_output_tokens,
                    capabilities = ?metadata.capabilities,
                    "model metadata resolved via prefix match"
                );
                return metadata.clone();
            }
        }
        let defaults = ModelMetadata {
            context_window: self.default_context_window,
            max_output_tokens: 4096,
            capabilities: BTreeSet::new(),
        };
        debug!(
            model = model_name,
            context_window = defaults.context_window,
            max_output_tokens = defaults.max_output_tokens,
            "no model metadata match — using defaults"
        );
        defaults
    }

    /// The conservative default context window when no model matches.
    pub fn default_context_window(&self) -> u32 {
        self.default_context_window
    }
}

impl Default for ModelMetadataRegistry {
    fn default() -> Self {
        Self::load()
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_yaml_loads() {
        let registry = ModelMetadataRegistry::load();
        assert!(!registry.entries.is_empty());
    }

    #[test]
    fn exact_match() {
        let registry = ModelMetadataRegistry::load();
        let meta = registry.lookup("gpt-4o");
        assert_eq!(meta.context_window, 128000);
        assert_eq!(meta.max_output_tokens, 16384);
    }

    #[test]
    fn prefix_match_with_version_suffix() {
        let registry = ModelMetadataRegistry::load();
        let meta = registry.lookup("gpt-4o-2024-08-06");
        assert_eq!(meta.context_window, 128000);
    }

    #[test]
    fn longest_prefix_wins() {
        let registry = ModelMetadataRegistry::load();
        // "gpt-4o-mini" should match before "gpt-4o"
        let meta = registry.lookup("gpt-4o-mini-2024-07-18");
        assert_eq!(meta.context_window, 128000);
        assert_eq!(meta.max_output_tokens, 16384);

        // "gpt-4.1-nano" should match before "gpt-4.1"
        let meta = registry.lookup("gpt-4.1-nano-2025-04-14");
        assert_eq!(meta.context_window, 1047576);
    }

    #[test]
    fn case_insensitive() {
        let registry = ModelMetadataRegistry::load();
        let meta = registry.lookup("GPT-4o");
        assert_eq!(meta.context_window, 128000);
    }

    #[test]
    fn unknown_model_returns_default() {
        let registry = ModelMetadataRegistry::load();
        let meta = registry.lookup("some-unknown-model-v99");
        assert_eq!(meta.context_window, 32768);
        assert!(meta.capabilities.is_empty());
    }

    #[test]
    fn anthropic_models() {
        let registry = ModelMetadataRegistry::load();

        let meta = registry.lookup("claude-sonnet-4.6");
        assert_eq!(meta.context_window, 200000);
        assert_eq!(meta.max_output_tokens, 64000);

        let meta = registry.lookup("claude-3.5-sonnet-20241022");
        assert_eq!(meta.context_window, 200000);
    }

    #[test]
    fn max_input_tokens_calculation() {
        let meta = ModelMetadata {
            context_window: 128000,
            max_output_tokens: 16384,
            capabilities: BTreeSet::new(),
        };
        assert_eq!(meta.max_input_tokens(), 111616);
    }

    #[test]
    fn gpt5_models() {
        let registry = ModelMetadataRegistry::load();

        let meta = registry.lookup("gpt-5.4");
        assert_eq!(meta.context_window, 1050000);
        assert_eq!(meta.max_output_tokens, 128000);

        let meta = registry.lookup("gpt-5.1-codex-mini");
        assert_eq!(meta.context_window, 400000);
        assert_eq!(meta.max_output_tokens, 100000);

        let meta = registry.lookup("gpt-5-mini");
        assert_eq!(meta.context_window, 400000);
        assert_eq!(meta.max_output_tokens, 128000);

        let meta = registry.lookup("gpt-5.3-codex");
        assert_eq!(meta.context_window, 400000);
    }

    #[test]
    fn phi_models() {
        let registry = ModelMetadataRegistry::load();

        let meta = registry.lookup("phi-4-mini-reasoning");
        assert_eq!(meta.context_window, 128000);
        assert_eq!(meta.max_output_tokens, 4096);

        let meta = registry.lookup("phi-4");
        assert_eq!(meta.context_window, 16000);
        assert_eq!(meta.max_output_tokens, 16000);

        let meta = registry.lookup("phi-3.5-mini");
        assert_eq!(meta.context_window, 128000);

        let meta = registry.lookup("phi-3-mini-128k-instruct");
        assert_eq!(meta.context_window, 128000);

        let meta = registry.lookup("phi-3-mini");
        assert_eq!(meta.context_window, 4096);
    }

    #[test]
    fn capabilities_from_yaml() {
        let registry = ModelMetadataRegistry::load();

        // GPT-4o should have full capabilities
        let meta = registry.lookup("gpt-4o");
        assert!(meta.capabilities.contains(&CapabilityConfig::Chat));
        assert!(meta.capabilities.contains(&CapabilityConfig::Code));
        assert!(meta.capabilities.contains(&CapabilityConfig::Vision));
        assert!(meta.capabilities.contains(&CapabilityConfig::ToolUse));

        // GPT-4 (base) should not have vision
        let meta = registry.lookup("gpt-4");
        assert!(meta.capabilities.contains(&CapabilityConfig::Chat));
        assert!(meta.capabilities.contains(&CapabilityConfig::ToolUse));
        assert!(!meta.capabilities.contains(&CapabilityConfig::Vision));

        // DeepSeek-R1 should have only chat + code
        let meta = registry.lookup("deepseek-r1");
        assert!(meta.capabilities.contains(&CapabilityConfig::Chat));
        assert!(meta.capabilities.contains(&CapabilityConfig::Code));
        assert!(!meta.capabilities.contains(&CapabilityConfig::ToolUse));
        assert!(!meta.capabilities.contains(&CapabilityConfig::Vision));

        // Unknown model should have empty capabilities
        let meta = registry.lookup("unknown-model-xyz");
        assert!(meta.capabilities.is_empty());
    }
}
