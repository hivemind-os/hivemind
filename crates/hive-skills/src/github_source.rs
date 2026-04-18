//! GitHub repo skill source — discovers skills by cloning repos.

use crate::parser;
use crate::scan;
use hive_contracts::{DiscoveredSkill, SkillSourceConfig};
use std::path::{Path, PathBuf};

/// A GitHub repository skill source that uses shallow git clone via gitoxide.
pub struct GitHubRepoSource {
    pub owner: String,
    pub repo: String,
    cache_dir: PathBuf,
}

impl GitHubRepoSource {
    pub fn new(owner: String, repo: String, cache_dir: PathBuf) -> Self {
        Self { owner, repo, cache_dir }
    }

    pub fn from_config(config: &SkillSourceConfig, cache_base: &Path) -> Option<Self> {
        match config {
            SkillSourceConfig::GitHub { owner, repo, enabled } if *enabled => {
                let cache_dir = cache_base.join(format!("{owner}_{repo}"));
                Some(Self::new(owner.clone(), repo.clone(), cache_dir))
            }
            _ => None,
        }
    }

    pub fn source_id(&self) -> String {
        format!("github:{}/{}", self.owner, self.repo)
    }

    /// Clone or update the repo in the cache directory.
    pub async fn sync(&self) -> Result<(), SourceError> {
        if self.cache_dir.join(".git").exists() {
            let cache_dir = self.cache_dir.clone();
            let source_id = self.source_id();
            let result = tokio::task::spawn_blocking(move || fetch_and_fast_forward(&cache_dir))
                .await
                .map_err(|e| SourceError::GitFailed(format!("task join error: {e}")))?;

            if let Err(e) = result {
                tracing::warn!("git fetch failed for {}, re-cloning: {}", source_id, e);
                tokio::fs::remove_dir_all(&self.cache_dir)
                    .await
                    .map_err(|e| SourceError::IoFailed(e.to_string()))?;
                self.clone_repo().await?;
            }
        } else {
            self.clone_repo().await?;
        }
        Ok(())
    }

    async fn clone_repo(&self) -> Result<(), SourceError> {
        if let Some(parent) = self.cache_dir.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SourceError::IoFailed(e.to_string()))?;
        }

        let url = format!("https://github.com/{}/{}.git", self.owner, self.repo);
        let cache_dir = self.cache_dir.clone();
        let source_id = self.source_id();

        tokio::task::spawn_blocking(move || shallow_clone(&url, &cache_dir))
            .await
            .map_err(|e| SourceError::GitFailed(format!("task join error: {e}")))?
            .map_err(|e| {
                SourceError::GitFailed(format!("git clone failed for {source_id}: {e}"))
            })?;

        Ok(())
    }

    /// Discover all skills in the cloned repo by scanning for SKILL.md files.
    pub async fn discover(&self) -> Result<Vec<DiscoveredSkill>, SourceError> {
        self.sync().await?;

        let mut skills = Vec::new();
        let source_id = self.source_id();
        scan::scan_directory(&self.cache_dir, &self.cache_dir, &source_id, &mut skills, 0).await;
        Ok(skills)
    }

    /// Fetch the full content of a specific skill by its source path.
    pub async fn fetch_skill_content(
        &self,
        source_path: &str,
    ) -> Result<hive_contracts::SkillContent, SourceError> {
        let skill_dir = self.cache_dir.join(source_path);
        let skill_md_path = skill_dir.join("SKILL.md");

        let skill_md = tokio::fs::read_to_string(&skill_md_path)
            .await
            .map_err(|e| SourceError::IoFailed(format!("failed to read SKILL.md: {e}")))?;

        let parsed = parser::parse_skill_md(&skill_md)
            .map_err(|e| SourceError::ParseFailed(e.to_string()))?;

        let mut files = std::collections::BTreeMap::new();
        scan::collect_skill_files(&skill_dir, &skill_dir, &mut files).await;

        Ok(hive_contracts::SkillContent { skill_md, body: parsed.body, files })
    }

    /// Return the HEAD commit SHA of the cached repo clone.
    ///
    /// This is used to pin the exact commit a skill was installed from so that
    /// upstream changes can be detected before they are silently applied.
    pub fn head_commit_sha(&self) -> Option<String> {
        let repo = gix::open(&self.cache_dir).ok()?;
        Some(repo.head_id().ok()?.to_string())
    }
}

/// Shallow-clone a repository using gix (depth=1, single branch).
fn shallow_clone(url: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parsed_url = gix::url::parse(url.into())?;
    let mut prepare_clone = gix::prepare_clone(parsed_url, dest)?
        .with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(1.try_into().expect("non-zero")));

    let (mut prepare_checkout, _outcome) = prepare_clone
        .fetch_then_checkout(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

    prepare_checkout.main_worktree(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

    Ok(())
}

/// Fetch from origin and fast-forward HEAD (equivalent to `git pull --ff-only`).
fn fetch_and_fast_forward(
    repo_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let repo = gix::open(repo_path)?;

    let remote = repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .ok_or("no default remote")?
        .map_err(|e| format!("failed to resolve remote: {e}"))?;

    remote
        .connect(gix::remote::Direction::Fetch)?
        .prepare_fetch(gix::progress::Discard, Default::default())?
        .receive(gix::progress::Discard, &gix::interrupt::IS_INTERRUPTED)?;

    // Fast-forward HEAD to the tracking branch tip
    let head = repo.head()?;
    if let Some(head_ref) = head.try_into_referent() {
        let ref_name = head_ref.name().as_bstr().to_string();
        let branch_name = ref_name.strip_prefix("refs/heads/").unwrap_or(&ref_name);

        let tracking_ref_name = format!("refs/remotes/origin/{branch_name}");
        if let Ok(tracking_ref) = repo.find_reference(&tracking_ref_name) {
            let remote_oid = tracking_ref.id();
            let local_oid = head_ref.id();

            if local_oid != remote_oid {
                // Update the HEAD ref
                let head_ref_name = head_ref.name().to_owned();
                repo.find_reference(head_ref_name.as_ref())?
                    .set_target_id(remote_oid, "fast-forward pull")?;

                // Reset the worktree to match the new commit
                reset_worktree(&repo, remote_oid.into())?;
            }
        }
    }

    Ok(())
}

/// Reset the worktree to match the given commit OID.
fn reset_worktree(
    repo: &gix::Repository,
    oid: gix::ObjectId,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let commit =
        repo.find_object(oid)?.try_into_commit().map_err(|e| format!("not a commit: {e}"))?;
    let tree_id = commit.tree_id()?;

    let index_state = gix::index::State::from_tree(&tree_id, &repo.objects, Default::default())?;
    let mut index = gix::index::File::from_state(index_state, repo.index_path());

    let workdir = repo.workdir().ok_or("no workdir")?;
    let opts = gix::worktree::state::checkout::Options {
        overwrite_existing: true,
        destination_is_initially_empty: false,
        ..Default::default()
    };

    gix::worktree::state::checkout(
        &mut index,
        workdir,
        repo.objects.clone(),
        &gix::progress::Discard,
        &gix::progress::Discard,
        &gix::interrupt::IS_INTERRUPTED,
        opts,
    )?;

    index.write(gix::index::write::Options::default())?;

    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum SourceError {
    #[error("git operation failed: {0}")]
    GitFailed(String),
    #[error("I/O error: {0}")]
    IoFailed(String),
    #[error("parse error: {0}")]
    ParseFailed(String),
}
