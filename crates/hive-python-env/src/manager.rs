//! Core Python environment manager.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::{download, platform, PythonEnvConfig, PythonEnvError};

/// Information about a ready Python environment.
#[derive(Debug, Clone)]
pub struct PythonEnvInfo {
    pub python_path: PathBuf,
    pub venv_path: PathBuf,
    pub uv_path: PathBuf,
}

/// Status of the managed Python environment.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PythonEnvStatus {
    Disabled,
    NotInstalled,
    Installing { progress: String },
    Ready { venv_path: String },
    Failed { error: String },
}

/// Manages the lifecycle of a curated Python environment for agents.
pub struct PythonEnvManager {
    hivemind_home: PathBuf,
    config: PythonEnvConfig,
    status: Arc<RwLock<PythonEnvStatus>>,
    status_tx: tokio::sync::broadcast::Sender<PythonEnvStatus>,
}

impl PythonEnvManager {
    pub fn new(hivemind_home: PathBuf, config: PythonEnvConfig) -> Self {
        let initial_status =
            if config.enabled { PythonEnvStatus::NotInstalled } else { PythonEnvStatus::Disabled };
        let (status_tx, _) = tokio::sync::broadcast::channel(16);
        Self { hivemind_home, config, status: Arc::new(RwLock::new(initial_status)), status_tx }
    }

    /// Directory where the uv binary is stored.
    fn uv_dir(&self) -> PathBuf {
        self.hivemind_home.join("runtimes").join("uv")
    }

    /// Path to the uv binary.
    fn uv_binary(&self) -> PathBuf {
        self.uv_dir().join(platform::uv_binary_name())
    }

    /// Path to the default managed virtual environment.
    fn default_venv_dir(&self) -> PathBuf {
        self.hivemind_home.join("runtimes").join("python").join("default")
    }

    /// Path to a session-scoped virtual environment.
    fn session_venv_dir(&self, session_id: &str) -> PathBuf {
        self.hivemind_home.join("runtimes").join("python").join("sessions").join(session_id)
    }

    /// Current status of the managed environment.
    pub async fn status(&self) -> PythonEnvStatus {
        self.status.read().await.clone()
    }

    /// Non-async status check (returns `None` if the lock is contended).
    pub fn status_blocking(&self) -> Option<PythonEnvStatus> {
        self.status.try_read().ok().map(|s| s.clone())
    }

    /// Subscribe to status change notifications.
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<PythonEnvStatus> {
        self.status_tx.subscribe()
    }

    /// Set status and broadcast the change.
    async fn set_status(&self, new_status: PythonEnvStatus) {
        *self.status.write().await = new_status.clone();
        let _ = self.status_tx.send(new_status);
    }

    /// Ensure the uv binary is available, downloading if needed.
    pub async fn ensure_uv(&self) -> Result<PathBuf, PythonEnvError> {
        if !self.config.enabled {
            return Err(PythonEnvError::Disabled);
        }
        download::ensure_uv_binary(&self.uv_dir(), &self.config.uv_version).await
    }

    /// Ensure the default managed Python venv exists and is up-to-date.
    ///
    /// This is the main setup entry point, typically called on daemon startup.
    pub async fn ensure_default_env(&self) -> Result<PythonEnvInfo, PythonEnvError> {
        if !self.config.enabled {
            return Err(PythonEnvError::Disabled);
        }

        self.set_status(PythonEnvStatus::Installing { progress: "downloading uv".to_string() })
            .await;
        tracing::info!("downloading uv package manager");

        let uv_path = match self.ensure_uv().await {
            Ok(p) => {
                tracing::info!(path = %p.display(), "uv binary ready");
                p
            }
            Err(e) => {
                tracing::error!(error = %e, "failed to download uv");
                self.set_status(PythonEnvStatus::Failed { error: e.to_string() }).await;
                return Err(e);
            }
        };

        let venv_dir = self.default_venv_dir();
        let python_path = venv_dir.join(platform::venv_python_relative());

        // If venv already exists with the right Python, check the manifest.
        if python_path.exists() && self.manifest_matches(&venv_dir) {
            let info = PythonEnvInfo { python_path, venv_path: venv_dir.clone(), uv_path };
            tracing::info!(venv = %venv_dir.display(), "existing Python environment is up-to-date");
            self.set_status(PythonEnvStatus::Ready {
                venv_path: venv_dir.to_string_lossy().to_string(),
            })
            .await;
            return Ok(info);
        }

        self.set_status(PythonEnvStatus::Installing { progress: "installing Python".to_string() })
            .await;
        tracing::info!(version = %self.config.python_version, "installing Python via uv");

        // Install Python via uv.
        if let Err(e) =
            self.run_uv(&uv_path, &["python", "install", &self.config.python_version]).await
        {
            tracing::error!(error = %e, "failed to install Python");
            self.set_status(PythonEnvStatus::Failed { error: e.to_string() }).await;
            return Err(e);
        }

        self.set_status(PythonEnvStatus::Installing {
            progress: "creating virtual environment".to_string(),
        })
        .await;
        tracing::info!(venv = %venv_dir.display(), "creating virtual environment");

        // Create the venv.
        std::fs::create_dir_all(venv_dir.parent().unwrap_or(Path::new(".")))?;
        if let Err(e) = self
            .run_uv(
                &uv_path,
                &["venv", &venv_dir.to_string_lossy(), "--python", &self.config.python_version],
            )
            .await
        {
            self.set_status(PythonEnvStatus::Failed { error: e.to_string() }).await;
            return Err(e);
        }

        // Install base packages.
        if !self.config.base_packages.is_empty() {
            self.set_status(PythonEnvStatus::Installing {
                progress: "installing base packages".to_string(),
            })
            .await;
            tracing::info!(count = self.config.base_packages.len(), "installing base packages");

            let python_str = python_path.to_string_lossy().to_string();
            let mut args: Vec<String> =
                vec!["pip".to_string(), "install".to_string(), "--python".to_string(), python_str];
            for pkg in &self.config.base_packages {
                args.push(pkg.clone());
            }
            let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

            if let Err(e) = self.run_uv(&uv_path, &arg_refs).await {
                tracing::error!(error = %e, "failed to install base packages");
                self.set_status(PythonEnvStatus::Failed { error: e.to_string() }).await;
                return Err(e);
            }
        }

        // Write a manifest so we know the venv matches this config.
        self.write_manifest(&venv_dir);

        let info = PythonEnvInfo { python_path, venv_path: venv_dir.clone(), uv_path };
        self.set_status(PythonEnvStatus::Ready {
            venv_path: venv_dir.to_string_lossy().to_string(),
        })
        .await;

        tracing::info!("managed Python environment ready at {}", venv_dir.display());
        Ok(info)
    }

    /// Return environment variables to inject into shell commands.
    ///
    /// Returns `None` if the environment is not ready.
    pub async fn shell_env_vars(
        &self,
        session_id: Option<&str>,
    ) -> Option<HashMap<String, String>> {
        let status = self.status.read().await;
        let venv_path = match &*status {
            PythonEnvStatus::Ready { venv_path } => PathBuf::from(venv_path),
            _ => return None,
        };

        // If a session-scoped venv exists, prefer it.
        let effective_venv = if let Some(sid) = session_id {
            let session_venv = self.session_venv_dir(sid);
            let session_python = session_venv.join(platform::venv_python_relative());
            if session_python.exists() {
                session_venv
            } else {
                venv_path
            }
        } else {
            venv_path
        };

        let bin_dir = effective_venv.join(platform::venv_bin_dir());
        let existing_path = std::env::var("PATH").unwrap_or_default();
        let new_path =
            format!("{}{}{}", bin_dir.to_string_lossy(), platform::path_separator(), existing_path);

        let mut vars = HashMap::new();
        vars.insert("PATH".to_string(), new_path);
        vars.insert("VIRTUAL_ENV".to_string(), effective_venv.to_string_lossy().to_string());
        Some(vars)
    }

    /// Install workspace-specific dependencies into a session-scoped venv.
    pub async fn install_workspace_deps(
        &self,
        session_id: &str,
        workspace_path: &Path,
    ) -> Result<Option<PythonEnvInfo>, PythonEnvError> {
        if !self.config.enabled || !self.config.auto_detect_workspace_deps {
            return Ok(None);
        }

        // Detect dependency files.
        let requirements = workspace_path.join("requirements.txt");
        let pyproject = workspace_path.join("pyproject.toml");

        let dep_file = if requirements.exists() {
            Some(requirements)
        } else if pyproject.exists() {
            Some(pyproject)
        } else {
            None
        };

        let dep_file = match dep_file {
            Some(f) => f,
            None => return Ok(None),
        };

        let uv_path = self.uv_binary();
        if !uv_path.exists() {
            return Err(PythonEnvError::UvCommand("uv binary not found".to_string()));
        }

        let session_venv = self.session_venv_dir(session_id);
        let python_path = session_venv.join(platform::venv_python_relative());

        // Create session venv if it doesn't exist.
        if !python_path.exists() {
            std::fs::create_dir_all(session_venv.parent().unwrap_or(Path::new(".")))?;
            self.run_uv(
                &uv_path,
                &["venv", &session_venv.to_string_lossy(), "--python", &self.config.python_version],
            )
            .await?;
        }

        // Install dependencies.
        let python_str = python_path.to_string_lossy().to_string();
        let dep_str = dep_file.to_string_lossy().to_string();

        let args = if dep_file.file_name().and_then(|n| n.to_str()) == Some("requirements.txt") {
            vec![
                "pip".to_string(),
                "install".to_string(),
                "--python".to_string(),
                python_str.clone(),
                "-r".to_string(),
                dep_str,
            ]
        } else {
            // For pyproject.toml, install the project in editable mode.
            let workspace_str = workspace_path.to_string_lossy().to_string();
            vec![
                "pip".to_string(),
                "install".to_string(),
                "--python".to_string(),
                python_str.clone(),
                "-e".to_string(),
                workspace_str,
            ]
        };

        let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.run_uv(&uv_path, &arg_refs).await?;

        tracing::info!(
            "installed workspace dependencies for session {session_id} from {}",
            dep_file.display()
        );

        Ok(Some(PythonEnvInfo { python_path, venv_path: session_venv, uv_path }))
    }

    /// Force-rebuild the managed environment.
    pub async fn reinstall(&self) -> Result<PythonEnvInfo, PythonEnvError> {
        let venv_dir = self.default_venv_dir();
        if venv_dir.exists() {
            std::fs::remove_dir_all(&venv_dir)?;
        }
        self.ensure_default_env().await
    }

    /// Run a uv command and check its exit status.
    async fn run_uv(&self, uv_path: &Path, args: &[&str]) -> Result<(), PythonEnvError> {
        tracing::debug!("running: {} {}", uv_path.display(), args.join(" "));

        let output = tokio::process::Command::new(uv_path)
            .args(args)
            .env("UV_NO_PROGRESS", "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| PythonEnvError::UvCommand(format!("failed to spawn uv: {e}")))?
            .wait_with_output()
            .await
            .map_err(|e| PythonEnvError::UvCommand(format!("failed to wait for uv: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            return Err(PythonEnvError::UvCommand(format!(
                "uv {} exited with {}: {}{}",
                args.first().unwrap_or(&""),
                output.status,
                stderr.trim(),
                if !stdout.trim().is_empty() {
                    format!("\n{}", stdout.trim())
                } else {
                    String::new()
                },
            )));
        }

        Ok(())
    }

    /// Check if the existing venv matches the current config.
    fn manifest_matches(&self, venv_dir: &Path) -> bool {
        let manifest_path = venv_dir.join(".hivemind-manifest.json");
        if let Ok(content) = std::fs::read_to_string(&manifest_path) {
            if let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&content) {
                let version_matches = manifest.get("python_version").and_then(|v| v.as_str())
                    == Some(&self.config.python_version);
                let packages_match = manifest
                    .get("base_packages")
                    .and_then(|v| serde_json::from_value::<Vec<String>>(v.clone()).ok())
                    .as_ref()
                    == Some(&self.config.base_packages);
                return version_matches && packages_match;
            }
        }
        false
    }

    /// Write a manifest recording the current config into the venv dir.
    fn write_manifest(&self, venv_dir: &Path) {
        let manifest = serde_json::json!({
            "python_version": self.config.python_version,
            "base_packages": self.config.base_packages,
            "uv_version": self.config.uv_version,
        });
        let manifest_path = venv_dir.join(".hivemind-manifest.json");
        if let Err(e) = std::fs::write(
            &manifest_path,
            serde_json::to_string_pretty(&manifest).unwrap_or_default(),
        ) {
            tracing::warn!("failed to write python env manifest: {e}");
        }
    }
}
