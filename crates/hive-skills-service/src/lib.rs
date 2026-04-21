mod skill_audit_prompt;

use arc_swap::ArcSwap;
use hive_contracts::{
    DiscoveredSkill, InstalledSkill, SkillAuditResult, SkillAuditRisk, SkillContent,
    SkillRiskSeverity, SkillSourceConfig, SkillsConfig,
};
use hive_model::{CompletionMessage, CompletionRequest, ModelRouter};
use hive_skills::{
    parse_skill_md, GitHubRepoSource, LocalDirSource, SkillCatalog, SkillIndex, SkillIndexStore,
};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

pub struct SkillsService {
    index: Arc<SkillIndex>,
    config: RwLock<SkillsConfig>,
    cache_dir: PathBuf,
    personas_dir: PathBuf,
    model_router: Option<Arc<ArcSwap<ModelRouter>>>,
}

impl SkillsService {
    pub fn new(
        config: SkillsConfig,
        data_dir: PathBuf,
        personas_dir: PathBuf,
        model_router: Option<Arc<ArcSwap<ModelRouter>>>,
    ) -> Result<Self, SkillsServiceError> {
        std::fs::create_dir_all(&data_dir).map_err(|error| SkillsServiceError::Io {
            path: data_dir.display().to_string(),
            detail: error.to_string(),
        })?;

        let cache_dir = data_dir.join("skills-cache");
        std::fs::create_dir_all(&cache_dir).map_err(|error| SkillsServiceError::Io {
            path: cache_dir.display().to_string(),
            detail: error.to_string(),
        })?;

        let index = Arc::new(SkillIndex::open(&data_dir.join("skills.db"))?);
        for source in &config.sources {
            index.save_source(source)?;
        }

        Ok(Self { index, config: RwLock::new(config), cache_dir, personas_dir, model_router })
    }

    pub async fn discover(
        &self,
        persona_id: Option<&str>,
    ) -> Result<Vec<DiscoveredSkill>, SkillsServiceError> {
        let config = self.config.read().await.clone();
        if !config.enabled {
            return Ok(Vec::new());
        }

        let mut all_skills = Vec::new();
        for source_config in &config.sources {
            if !source_config.is_enabled() {
                continue;
            }
            match source_config {
                SkillSourceConfig::GitHub { .. } => {
                    if let Some(source) =
                        GitHubRepoSource::from_config(source_config, &self.cache_dir)
                    {
                        match source.discover().await {
                            Ok(skills) => all_skills.extend(skills),
                            Err(error) => {
                                tracing::warn!(
                                    "Failed to discover from {}: {}",
                                    source.source_id(),
                                    error
                                );
                            }
                        }
                    }
                }
                SkillSourceConfig::LocalDirectory { .. } => {
                    if let Some(source) = LocalDirSource::from_config(source_config) {
                        match source.discover().await {
                            Ok(skills) => all_skills.extend(skills),
                            Err(error) => {
                                tracing::warn!(
                                    "Failed to discover from {}: {}",
                                    source.source_id(),
                                    error
                                );
                            }
                        }
                    }
                }
            }
        }

        self.index.insert_discovered(&all_skills)?;
        Ok(self.index.list_discovered(persona_id)?)
    }

    pub async fn list_installed(
        &self,
        persona_id: &str,
    ) -> Result<Vec<InstalledSkill>, SkillsServiceError> {
        Ok(self.index.list_installed(Some(persona_id))?)
    }

    /// Ensure a bundled skill is present in the index. If the skill is not
    /// already in the DB for the given persona, insert it with a synthetic
    /// audit result. If it already exists, leave it untouched (preserving
    /// user enable/disable choices).
    pub fn sync_bundled_skill(
        &self,
        persona_id: &str,
        manifest: hive_contracts::SkillManifest,
        local_path: &str,
    ) -> Result<(), SkillsServiceError> {
        let existing = self.index.get_installed(&manifest.name, Some(persona_id))?;
        if existing.is_some() {
            return Ok(());
        }

        let now_ms =
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64;

        // Compute content hash from on-disk files for bundled skills
        let content_hash = compute_on_disk_hash(Path::new(local_path));

        let skill = InstalledSkill {
            manifest,
            local_path: local_path.to_string(),
            source_id: "bundled".to_string(),
            source_path: String::new(),
            persona_id: persona_id.to_string(),
            audit: SkillAuditResult {
                model_used: "n/a".to_string(),
                risks: Vec::new(),
                summary: "Bundled skill — pre-audited".to_string(),
                audited_at_ms: now_ms,
            },
            enabled: true,
            installed_at_ms: now_ms,
            content_hash,
            pinned_commit: String::new(),
        };
        self.index.install_skill(&skill)?;
        Ok(())
    }

    pub async fn list_discovered(
        &self,
        persona_id: Option<&str>,
    ) -> Result<Vec<DiscoveredSkill>, SkillsServiceError> {
        Ok(self.index.list_discovered(persona_id)?)
    }

    pub async fn audit_skill(
        &self,
        source_id: &str,
        source_path: &str,
        skill_content: &SkillContent,
        model: &str,
    ) -> Result<SkillAuditResult, SkillsServiceError> {
        let router = self.model_router.as_ref().ok_or_else(|| {
            SkillsServiceError::Config("model router not configured for skill auditing".into())
        })?;

        let system_prompt = skill_audit_prompt::skill_audit_system_prompt().to_string();
        let user_message = skill_audit_prompt::format_skill_audit_payload(
            &skill_content.skill_md,
            source_id,
            source_path,
            &skill_content.files,
        );

        let request = CompletionRequest {
            prompt: user_message,
            prompt_content_parts: vec![],
            messages: vec![CompletionMessage {
                role: "system".to_string(),
                content: system_prompt,
                content_parts: vec![],
            }],
            required_capabilities: BTreeSet::new(),
            preferred_models: Some(vec![model.to_string()]),
            tools: vec![],
        };

        let router_guard = router.load();
        let response = router_guard
            .complete(&request)
            .map_err(|e| SkillsServiceError::Audit(format!("model call failed: {e}")))?;

        parse_audit_response(&response.content, model)
    }

    pub async fn install_skill(
        &self,
        name: &str,
        source_id: &str,
        source_path: &str,
        persona_id: &str,
        audit: SkillAuditResult,
    ) -> Result<InstalledSkill, SkillsServiceError> {
        validate_skill_name(name)?;
        let content = self.fetch_skill_content(source_id, source_path).await?;
        let parsed = parse_skill_md(&content.skill_md)
            .map_err(|error| SkillsServiceError::Parse(error.to_string()))?;

        // Compute content hash for integrity verification
        let content_hash = content.content_hash();

        // Read the pinned commit from the source repo
        let pinned_commit = self
            .source_for_id(source_id)
            .await
            .and_then(|s| s.head_commit_sha())
            .unwrap_or_default();

        if persona_id.is_empty() {
            return Err(SkillsServiceError::Config(
                "persona_id is required for skill installation".into(),
            ));
        }
        let persona_path = persona_id.replace('/', std::path::MAIN_SEPARATOR_STR);
        let install_dir = self.personas_dir.join(persona_path).join("skills").join(name);

        if tokio::fs::try_exists(&install_dir).await.map_err(|error| SkillsServiceError::Io {
            path: install_dir.display().to_string(),
            detail: error.to_string(),
        })? {
            tokio::fs::remove_dir_all(&install_dir).await.map_err(|error| {
                SkillsServiceError::Io {
                    path: install_dir.display().to_string(),
                    detail: error.to_string(),
                }
            })?;
        }
        tokio::fs::create_dir_all(&install_dir).await.map_err(|error| SkillsServiceError::Io {
            path: install_dir.display().to_string(),
            detail: error.to_string(),
        })?;

        tokio::fs::write(install_dir.join("SKILL.md"), &content.skill_md).await.map_err(
            |error| SkillsServiceError::Io {
                path: install_dir.join("SKILL.md").display().to_string(),
                detail: error.to_string(),
            },
        )?;

        for (relative_path, file_content) in content.files {
            validate_relative_path(&relative_path)?;
            let destination = install_dir.join(&relative_path);
            if let Some(parent) = destination.parent() {
                tokio::fs::create_dir_all(parent).await.map_err(|error| {
                    SkillsServiceError::Io {
                        path: parent.display().to_string(),
                        detail: error.to_string(),
                    }
                })?;
            }
            tokio::fs::write(&destination, file_content).await.map_err(|error| {
                SkillsServiceError::Io {
                    path: destination.display().to_string(),
                    detail: error.to_string(),
                }
            })?;
        }

        let installed = InstalledSkill {
            manifest: parsed.manifest,
            local_path: install_dir.display().to_string(),
            source_id: source_id.to_string(),
            source_path: source_path.to_string(),
            persona_id: persona_id.to_string(),
            audit,
            enabled: true,
            installed_at_ms: now_ms(),
            content_hash,
            pinned_commit,
        };

        self.index.install_skill(&installed)?;
        Ok(installed)
    }

    pub async fn uninstall_skill(
        &self,
        name: &str,
        persona_id: &str,
    ) -> Result<bool, SkillsServiceError> {
        let installed = self.index.get_installed(name, Some(persona_id))?;
        let removed = self.index.uninstall_skill(name, Some(persona_id))?;
        if removed {
            if let Some(skill) = installed {
                let local_path = PathBuf::from(skill.local_path);
                if tokio::fs::try_exists(&local_path).await.map_err(|error| {
                    SkillsServiceError::Io {
                        path: local_path.display().to_string(),
                        detail: error.to_string(),
                    }
                })? {
                    tokio::fs::remove_dir_all(&local_path).await.map_err(|error| {
                        SkillsServiceError::Io {
                            path: local_path.display().to_string(),
                            detail: error.to_string(),
                        }
                    })?;
                }
            }
        }
        Ok(removed)
    }

    pub async fn set_skill_enabled(
        &self,
        name: &str,
        persona_id: &str,
        enabled: bool,
    ) -> Result<bool, SkillsServiceError> {
        let updated = self.index.set_skill_enabled(name, Some(persona_id), enabled)?;
        Ok(updated)
    }

    pub async fn rebuild_index(&self) -> Result<Vec<DiscoveredSkill>, SkillsServiceError> {
        self.index.clear_discovered()?;
        self.discover(None).await
    }

    /// Build a catalog containing only skills installed for a specific persona.
    pub async fn catalog_for_persona(
        &self,
        persona_id: &str,
    ) -> Result<Arc<SkillCatalog>, SkillsServiceError> {
        let installed = {
            let config = self.config.read().await;
            if config.enabled {
                self.index.list_enabled(Some(persona_id))?
            } else {
                Vec::new()
            }
        };
        Ok(Arc::new(SkillCatalog::new(installed)))
    }

    /// Rebuild a catalog scoped to a specific persona.
    pub async fn rebuild_catalog_for_persona(
        &self,
        persona_id: &str,
    ) -> Result<Arc<SkillCatalog>, SkillsServiceError> {
        self.catalog_for_persona(persona_id).await
    }

    pub async fn get_sources(&self) -> Vec<SkillSourceConfig> {
        self.config.read().await.sources.clone()
    }

    pub async fn update_config(&self, config: SkillsConfig) -> Result<(), SkillsServiceError> {
        {
            let mut current = self.config.write().await;
            *current = config.clone();
        }
        for source in &config.sources {
            self.index.save_source(source)?;
        }
        Ok(())
    }

    pub async fn set_sources(
        &self,
        sources: Vec<SkillSourceConfig>,
    ) -> Result<Vec<SkillSourceConfig>, SkillsServiceError> {
        {
            let mut config = self.config.write().await;
            config.sources = sources.clone();
        }
        for source in &sources {
            self.index.save_source(source)?;
        }
        Ok(sources)
    }

    /// Verify integrity of all installed skills for a persona.
    ///
    /// Re-reads each skill's files from disk, computes the content hash, and
    /// compares it against the stored hash. Skills that fail verification are
    /// automatically disabled and returned in the result list so the caller
    /// (e.g., startup or catalog rebuild) can alert the user.
    pub async fn verify_installed_integrity(
        &self,
        persona_id: &str,
    ) -> Result<Vec<InstalledSkill>, SkillsServiceError> {
        let installed = self.index.list_installed(Some(persona_id))?;
        let mut tampered = Vec::new();

        for skill in &installed {
            // Skip skills with no stored hash (legacy installs before this feature)
            if skill.content_hash.is_empty() {
                continue;
            }

            let current_hash = compute_on_disk_hash(Path::new(&skill.local_path));
            if current_hash != skill.content_hash {
                tracing::warn!(
                    skill = %skill.manifest.name,
                    persona = %persona_id,
                    expected = %skill.content_hash,
                    actual = %current_hash,
                    "skill content hash mismatch — disabling skill"
                );
                let _ = self.index.set_skill_enabled(&skill.manifest.name, Some(persona_id), false);
                tampered.push(skill.clone());
            }
        }

        Ok(tampered)
    }

    pub async fn fetch_skill_content(
        &self,
        source_id: &str,
        source_path: &str,
    ) -> Result<SkillContent, SkillsServiceError> {
        let source = self
            .source_for_id(source_id)
            .await
            .ok_or_else(|| SkillsServiceError::SourceNotFound(source_id.to_string()))?;
        match source {
            AnySkillSource::GitHub(s) => {
                s.fetch_skill_content(source_path).await.map_err(Into::into)
            }
            AnySkillSource::Local(s) => {
                s.fetch_skill_content(source_path).await.map_err(Into::into)
            }
        }
    }

    async fn source_for_id(&self, source_id: &str) -> Option<AnySkillSource> {
        let config = self.config.read().await.clone();
        config.sources.iter().find_map(|source_config| {
            if source_config.source_id() != source_id {
                return None;
            }
            match source_config {
                SkillSourceConfig::GitHub { owner, repo, .. } => {
                    Some(AnySkillSource::GitHub(GitHubRepoSource::new(
                        owner.clone(),
                        repo.clone(),
                        self.cache_dir.join(format!("{owner}_{repo}")),
                    )))
                }
                SkillSourceConfig::LocalDirectory { path, .. } => {
                    let p = PathBuf::from(path);
                    if p.is_absolute() && p.is_dir() {
                        Some(AnySkillSource::Local(LocalDirSource::new(p)))
                    } else {
                        None
                    }
                }
            }
        })
    }
}

/// Wrapper enum so the service can work with either source type.
enum AnySkillSource {
    GitHub(GitHubRepoSource),
    Local(LocalDirSource),
}

impl AnySkillSource {
    fn head_commit_sha(&self) -> Option<String> {
        match self {
            Self::GitHub(s) => s.head_commit_sha(),
            Self::Local(s) => s.head_commit_sha(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SkillsServiceError {
    #[error(transparent)]
    Index(#[from] hive_skills::index::IndexError),
    #[error(transparent)]
    Source(#[from] hive_skills::github_source::SourceError),
    #[error(transparent)]
    LocalSource(#[from] hive_skills::local_dir_source::LocalSourceError),
    #[error("I/O error for {path}: {detail}")]
    Io { path: String, detail: String },
    #[error("source `{0}` was not found")]
    SourceNotFound(String),
    #[error("invalid skill content path `{0}`")]
    InvalidPath(String),
    #[error("failed to parse skill content: {0}")]
    Parse(String),
    #[error("skill audit failed: {0}")]
    Audit(String),
    #[error("configuration error: {0}")]
    Config(String),
}

/// Parse the model's JSON response into a `SkillAuditResult`.
/// If parsing fails, return a "suspicious" result rather than silently passing.
fn parse_audit_response(
    content: &str,
    model: &str,
) -> Result<SkillAuditResult, SkillsServiceError> {
    // Strip markdown code fences if the model wrapped the response.
    let trimmed = content.trim();
    let json_str = if trimmed.starts_with("```") {
        let without_opening = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        without_opening.strip_suffix("```").unwrap_or(without_opening).trim()
    } else {
        trimmed
    };

    #[derive(serde::Deserialize)]
    struct RawAuditResponse {
        #[serde(default)]
        risks: Vec<RawRisk>,
        #[serde(default)]
        summary: String,
    }

    #[derive(serde::Deserialize)]
    struct RawRisk {
        #[serde(default)]
        id: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        severity: String,
        #[serde(default)]
        evidence: Option<String>,
    }

    match serde_json::from_str::<RawAuditResponse>(json_str) {
        Ok(raw) => {
            let risks = raw
                .risks
                .into_iter()
                .map(|r| {
                    let severity = match r.severity.to_lowercase().as_str() {
                        "critical" => SkillRiskSeverity::Critical,
                        "high" => SkillRiskSeverity::High,
                        "medium" => SkillRiskSeverity::Medium,
                        _ => SkillRiskSeverity::Low,
                    };
                    SkillAuditRisk {
                        id: r.id,
                        description: r.description,
                        probability: match severity {
                            SkillRiskSeverity::Critical => 0.95,
                            SkillRiskSeverity::High => 0.8,
                            SkillRiskSeverity::Medium => 0.5,
                            SkillRiskSeverity::Low => 0.3,
                        },
                        severity,
                        evidence: r.evidence,
                    }
                })
                .collect();

            Ok(SkillAuditResult {
                model_used: model.to_string(),
                risks,
                summary: raw.summary,
                audited_at_ms: now_ms(),
            })
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "failed to parse skill audit response — treating as suspicious"
            );
            Ok(SkillAuditResult {
                model_used: model.to_string(),
                risks: vec![SkillAuditRisk {
                    id: "unparseable_audit".to_string(),
                    description: format!(
                        "The audit model returned a response that could not be parsed: {e}"
                    ),
                    probability: 0.7,
                    severity: SkillRiskSeverity::High,
                    evidence: Some(content.chars().take(500).collect::<String>()),
                }],
                summary: "Audit response could not be parsed. Treating skill as suspicious."
                    .to_string(),
                audited_at_ms: now_ms(),
            })
        }
    }
}

/// Re-compute the content hash from on-disk files for an installed skill directory.
///
/// Mirrors the logic of `SkillContent::content_hash()`: hashes SKILL.md first,
/// then all text-like supporting files in sorted order with length-prefixed framing.
fn compute_on_disk_hash(skill_dir: &Path) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();

    // Read SKILL.md
    let skill_md = match std::fs::read_to_string(skill_dir.join("SKILL.md")) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    hasher.update((skill_md.len() as u64).to_le_bytes());
    hasher.update(skill_md.as_bytes());

    // Collect supporting files in sorted order (same as BTreeMap iteration)
    let mut files = std::collections::BTreeMap::new();
    collect_on_disk_files(skill_dir, skill_dir, &mut files);
    for (path, body) in &files {
        hasher.update((path.len() as u64).to_le_bytes());
        hasher.update(path.as_bytes());
        hasher.update((body.len() as u64).to_le_bytes());
        hasher.update(body.as_bytes());
    }

    format!("{:x}", hasher.finalize())
}

/// Recursively collect text files under a skill directory (synchronous, for startup).
fn collect_on_disk_files(
    base: &Path,
    dir: &Path,
    files: &mut std::collections::BTreeMap<String, String>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    const SKIP_DIRS: &[&str] = &[".git", "node_modules", "__pycache__", ".venv", "target"];
    const TEXT_EXTS: &[&str] = &[
        "md", "txt", "py", "js", "ts", "sh", "bash", "yaml", "yml", "json", "toml", "rs", "go",
        "rb", "pl", "r", "sql", "html", "css", "xml", "csv", "cfg", "ini", "conf",
    ];

    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        if path.is_file() && name != "SKILL.md" {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_text = TEXT_EXTS.contains(&ext) || ext.is_empty();
            if is_text {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    // Use forward slashes for cross-platform consistency
                    let rel = path
                        .strip_prefix(base)
                        .unwrap_or(&path)
                        .to_string_lossy()
                        .replace('\\', "/");
                    files.insert(rel, content);
                }
            }
        } else if path.is_dir() && !SKIP_DIRS.contains(&name.as_str()) {
            collect_on_disk_files(base, &path, files);
        }
    }
}

fn validate_relative_path(path: &str) -> Result<(), SkillsServiceError> {
    let candidate = Path::new(path);
    if candidate.components().all(|component| matches!(component, Component::Normal(_))) {
        Ok(())
    } else {
        Err(SkillsServiceError::InvalidPath(path.to_string()))
    }
}

fn validate_skill_name(name: &str) -> Result<(), SkillsServiceError> {
    let candidate = Path::new(name);
    if candidate.components().count() == 1
        && candidate.components().all(|component| matches!(component, Component::Normal(_)))
    {
        Ok(())
    } else {
        Err(SkillsServiceError::InvalidPath(name.to_string()))
    }
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}
