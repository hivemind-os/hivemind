use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

// ── Skill Manifest (parsed from SKILL.md frontmatter) ──────────────

/// Metadata parsed from a SKILL.md YAML frontmatter block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillManifest {
    /// Unique skill name (1-64 chars, lowercase alphanumeric + hyphens).
    pub name: String,
    /// What the skill does and when to use it (1-1024 chars).
    pub description: String,
    /// License name or reference to a bundled license file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    /// Environment requirements (intended product, system packages, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatibility: Option<String>,
    /// Arbitrary key-value metadata.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, String>,
    /// Space-delimited list of pre-approved tools (experimental).
    #[serde(default, skip_serializing_if = "Option::is_none", alias = "allowedTools")]
    pub allowed_tools: Option<String>,
}

// ── Discovered Skill (from a source scan) ──────────────────────────

/// A skill discovered from a remote source but not yet installed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveredSkill {
    /// Parsed manifest from SKILL.md frontmatter.
    pub manifest: SkillManifest,
    /// Identifier of the source this skill was discovered from.
    #[serde(alias = "sourceId")]
    pub source_id: String,
    /// Relative path within the source (e.g. "skills/pdf-processing").
    #[serde(alias = "sourcePath")]
    pub source_path: String,
    /// Whether this skill is already installed locally.
    #[serde(default)]
    pub installed: bool,
}

// ── Installed Skill ────────────────────────────────────────────────

/// A skill that has been audited and installed locally.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InstalledSkill {
    /// Parsed manifest.
    pub manifest: SkillManifest,
    /// Absolute path to the local skill directory.
    #[serde(alias = "localPath")]
    pub local_path: String,
    /// Source it was installed from.
    #[serde(alias = "sourceId")]
    pub source_id: String,
    /// Relative path within the source.
    #[serde(alias = "sourcePath")]
    pub source_path: String,
    /// Persona this skill is installed for (e.g. "system/general").
    #[serde(default, alias = "personaId")]
    pub persona_id: String,
    /// Security audit results at time of installation.
    pub audit: SkillAuditResult,
    /// Whether the skill is enabled for use by agents.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Epoch millis when the skill was installed.
    #[serde(alias = "installedAtMs")]
    pub installed_at_ms: u64,
    /// SHA-256 hash of all skill content at install time (hex-encoded).
    /// Used to detect tampering or unexpected changes.
    #[serde(default, alias = "contentHash")]
    pub content_hash: String,
    /// Git commit SHA the skill was installed from (empty for bundled skills).
    #[serde(default, alias = "pinnedCommit")]
    pub pinned_commit: String,
}

fn default_true() -> bool {
    true
}

// ── Security Audit ─────────────────────────────────────────────────

/// Result of an LLM security audit on a skill's content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillAuditResult {
    /// The model used for the audit.
    #[serde(alias = "modelUsed")]
    pub model_used: String,
    /// Individual risks identified.
    pub risks: Vec<SkillAuditRisk>,
    /// Overall summary from the auditor.
    pub summary: String,
    /// Epoch millis when the audit was performed.
    #[serde(alias = "auditedAtMs")]
    pub audited_at_ms: u64,
}

/// A single risk identified during a skill security audit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SkillAuditRisk {
    /// Short identifier (e.g. "prompt-injection", "data-exfiltration").
    pub id: String,
    /// Human-readable description of the risk.
    pub description: String,
    /// Probability that this risk is real (0.0 – 1.0).
    pub probability: f64,
    /// Severity if exploited.
    pub severity: SkillRiskSeverity,
    /// The specific content that triggered this risk finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
}

/// Severity level for a skill audit risk.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SkillRiskSeverity {
    Low,
    Medium,
    High,
    Critical,
}

// ── Skill Source Config ────────────────────────────────────────────

/// Configuration for a source of skills (e.g. a GitHub repo).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillSourceConfig {
    /// A GitHub repository containing skills with SKILL.md files.
    #[serde(rename = "github")]
    GitHub {
        /// Repository owner (e.g. "anthropics").
        owner: String,
        /// Repository name (e.g. "skills").
        repo: String,
        /// Whether this source is enabled for discovery.
        #[serde(default = "default_true")]
        enabled: bool,
    },
    // Future: OCI registry, local directory, etc.
}

impl SkillSourceConfig {
    /// Unique identifier for this source.
    pub fn source_id(&self) -> String {
        match self {
            Self::GitHub { owner, repo, .. } => format!("github:{owner}/{repo}"),
        }
    }

    pub fn is_enabled(&self) -> bool {
        match self {
            Self::GitHub { enabled, .. } => *enabled,
        }
    }
}

// ── Skills Config (top-level section in HiveMindConfig) ───────────────

/// Configuration for the agent skills system.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SkillsConfig {
    /// Whether the skills system is enabled.
    pub enabled: bool,
    /// Configured skill sources.
    pub sources: Vec<SkillSourceConfig>,
    /// Custom local storage path for installed skills (deprecated – skills are
    /// now stored per-persona under the personas directory).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub storage_path: Option<String>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            sources: vec![
                SkillSourceConfig::GitHub {
                    owner: "anthropics".to_string(),
                    repo: "skills".to_string(),
                    enabled: true,
                },
                SkillSourceConfig::GitHub {
                    owner: "openai".to_string(),
                    repo: "skills".to_string(),
                    enabled: true,
                },
                SkillSourceConfig::GitHub {
                    owner: "huggingface".to_string(),
                    repo: "skills".to_string(),
                    enabled: true,
                },
            ],
            storage_path: None,
        }
    }
}

// ── Skill Content (full content for audit / installation) ──────────

/// Full content of a skill, fetched from a source for audit/install.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillContent {
    /// The raw SKILL.md file content.
    pub skill_md: String,
    /// The parsed body (markdown after frontmatter).
    pub body: String,
    /// Additional files in the skill directory (relative path → content).
    pub files: BTreeMap<String, String>,
}

impl SkillContent {
    /// Compute a deterministic SHA-256 hash over all skill content.
    ///
    /// The hash covers SKILL.md followed by every supporting file in sorted
    /// key order, with length-prefixed framing to prevent ambiguity.
    /// Path keys are normalized to forward slashes for cross-platform consistency.
    pub fn content_hash(&self) -> String {
        let mut hasher = Sha256::new();
        // Hash SKILL.md with length prefix
        hasher.update((self.skill_md.len() as u64).to_le_bytes());
        hasher.update(self.skill_md.as_bytes());
        // Hash supporting files in deterministic (sorted) order.
        // Normalize keys to forward slashes so the hash is stable across platforms.
        let normalized: BTreeMap<String, &String> =
            self.files.iter().map(|(k, v)| (k.replace('\\', "/"), v)).collect();
        for (path, body) in &normalized {
            hasher.update((path.len() as u64).to_le_bytes());
            hasher.update(path.as_bytes());
            hasher.update((body.len() as u64).to_le_bytes());
            hasher.update(body.as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }
}
