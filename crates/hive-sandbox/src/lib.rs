//! OS-level sandboxing for child processes.
//!
//! Provides a platform-agnostic [`Sandbox`] trait that wraps shell command
//! execution with filesystem and network restrictions:
//!
//! - **macOS**: `sandbox-exec` with generated Seatbelt profiles
//! - **Linux**: Landlock (kernel 5.13+) with bubblewrap fallback
//! - **Windows**: Restricted tokens (Low integrity) + Job Objects
//! - **Fallback**: Noop passthrough when sandboxing is unavailable

mod noop;
mod policy;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(target_os = "windows")]
mod windows_sandbox;

pub use policy::{AccessMode, AllowedPath, PolicyBuilder, SandboxPolicy};

use std::path::PathBuf;

/// Result of attempting to create a sandboxed command.
pub enum SandboxedCommand {
    /// The command string has been rewritten to execute under a sandbox.
    Wrapped {
        /// The program to execute (e.g. `sandbox-exec`, `bwrap`, or `sh`).
        program: String,
        /// Arguments to the program.
        args: Vec<String>,
        /// Temp files that must be kept alive until the process exits.
        _temp_files: Vec<tempfile::TempPath>,
    },
    /// Sandboxing is not available; run the original command unsandboxed.
    Passthrough,
}

/// Errors that can occur during sandbox setup.
#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("sandbox profile generation failed: {0}")]
    ProfileGeneration(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sandbox mechanism not available: {0}")]
    Unavailable(String),
}

/// Configuration for the sandbox feature (mirrors HiveMindConfig).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Enable OS-level sandboxing for shell commands.
    pub enabled: bool,
    /// Additional paths to allow read access.
    pub extra_read_paths: Vec<String>,
    /// Additional paths to allow read-write access.
    pub extra_write_paths: Vec<String>,
    /// Allow network access in sandboxed commands.
    pub allow_network: bool,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extra_read_paths: Vec::new(),
            extra_write_paths: Vec::new(),
            allow_network: true,
        }
    }
}

/// Build a sandboxed command for the current platform.
///
/// Returns [`SandboxedCommand::Wrapped`] with the rewritten command if
/// sandboxing is available, or [`SandboxedCommand::Passthrough`] if not.
///
/// The optional `shell` parameter overrides the default shell used inside
/// the sandbox wrapper. When `None`, the platform default is used
/// (`sh` on Unix, `cmd` on Windows).
pub fn sandbox_command(
    command: &str,
    policy: &SandboxPolicy,
) -> Result<SandboxedCommand, SandboxError> {
    sandbox_command_with_shell(command, policy, None, None)
}

/// Like [`sandbox_command`] but allows specifying a custom shell program and
/// command flag to use inside the sandbox wrapper.
pub fn sandbox_command_with_shell(
    command: &str,
    policy: &SandboxPolicy,
    shell_program: Option<&str>,
    shell_flag: Option<&str>,
) -> Result<SandboxedCommand, SandboxError> {
    #[cfg(target_os = "macos")]
    {
        match macos::sandbox_command_with_shell(command, policy, shell_program, shell_flag) {
            Ok(cmd) => return Ok(cmd),
            Err(e) => {
                tracing::warn!(error = %e, "macOS sandbox unavailable, falling back to unsandboxed");
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        match linux::sandbox_command_with_shell(command, policy, shell_program, shell_flag) {
            Ok(cmd) => return Ok(cmd),
            Err(e) => {
                tracing::warn!(error = %e, "Linux sandbox unavailable, falling back to unsandboxed");
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        match windows_sandbox::sandbox_command_with_shell(
            command,
            policy,
            shell_program,
            shell_flag,
        ) {
            Ok(cmd) => return Ok(cmd),
            Err(e) => {
                tracing::warn!(error = %e, "Windows sandbox unavailable, falling back to unsandboxed");
            }
        }
    }

    Ok(SandboxedCommand::Passthrough)
}

/// Well-known sensitive directories to deny by default.
pub fn default_denied_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs_home() {
        for name in &[".ssh", ".aws", ".gnupg", ".kube", ".azure", ".config/gcloud"] {
            let p = home.join(name);
            if p.exists() {
                paths.push(p);
            }
        }
    }
    paths
}

/// Well-known system paths needed for shell/Python to function.
pub fn default_system_read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "macos") {
        for p in &[
            "/usr",
            "/bin",
            "/sbin",
            "/Library",
            "/System",
            "/private/tmp",
            "/dev",
            "/etc",
            "/var/folders",
            "/tmp",
            "/private/var/folders",
        ] {
            paths.push(PathBuf::from(p));
        }
        // Homebrew paths (Apple Silicon and Intel)
        for p in &["/opt/homebrew", "/usr/local"] {
            let pb = PathBuf::from(p);
            if pb.exists() {
                paths.push(pb);
            }
        }
    } else if cfg!(target_os = "linux") {
        for p in
            &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/dev", "/tmp", "/proc", "/sys"]
        {
            paths.push(PathBuf::from(p));
        }
    } else if cfg!(target_os = "windows") {
        if let Ok(sysroot) = std::env::var("SystemRoot") {
            paths.push(PathBuf::from(&sysroot));
        }
        if let Ok(progfiles) = std::env::var("ProgramFiles") {
            paths.push(PathBuf::from(&progfiles));
        }
        if let Ok(progfiles86) = std::env::var("ProgramFiles(x86)") {
            paths.push(PathBuf::from(&progfiles86));
        }
    }
    paths
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE").ok().map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_enabled() {
        let cfg = SandboxConfig::default();
        assert!(cfg.enabled);
        assert!(cfg.allow_network);
        assert!(cfg.extra_read_paths.is_empty());
    }

    #[test]
    fn default_system_read_paths_non_empty() {
        let paths = default_system_read_paths();
        assert!(!paths.is_empty());
    }

    #[test]
    fn passthrough_when_disabled() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let result = sandbox_command("echo hello", &policy).unwrap();
        // On CI / test environments, sandbox may or may not be available.
        // The function should never error — it either wraps or passes through.
        match result {
            SandboxedCommand::Wrapped { .. } | SandboxedCommand::Passthrough => {}
        }
    }
}
