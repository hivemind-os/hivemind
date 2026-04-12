use std::collections::HashMap;
use std::path::PathBuf;

/// Describes what a sandboxed process is allowed to access.
#[derive(Debug, Clone)]
pub struct SandboxPolicy {
    /// Paths the process may access, each with a read/write mode.
    pub allowed_paths: Vec<AllowedPath>,
    /// Paths explicitly denied (overrides any allow).
    pub denied_paths: Vec<PathBuf>,
    /// Whether the process may use the network.
    pub allow_network: bool,
    /// Extra environment variables for the sandboxed process.
    pub env_overrides: HashMap<String, String>,
}

/// A single allowed path with its access mode.
#[derive(Debug, Clone)]
pub struct AllowedPath {
    pub path: PathBuf,
    pub mode: AccessMode,
}

/// Access mode for an allowed path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessMode {
    ReadOnly,
    ReadWrite,
}

impl SandboxPolicy {
    /// Create a builder for constructing a policy.
    pub fn builder() -> PolicyBuilder {
        PolicyBuilder::default()
    }
}

/// Fluent builder for [`SandboxPolicy`].
#[derive(Default)]
pub struct PolicyBuilder {
    allowed: Vec<AllowedPath>,
    denied: Vec<PathBuf>,
    allow_network: bool,
    env: HashMap<String, String>,
}

impl PolicyBuilder {
    pub fn allow_read(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed.push(AllowedPath { path: path.into(), mode: AccessMode::ReadOnly });
        self
    }

    pub fn allow_read_write(mut self, path: impl Into<PathBuf>) -> Self {
        self.allowed.push(AllowedPath { path: path.into(), mode: AccessMode::ReadWrite });
        self
    }

    pub fn deny(mut self, path: impl Into<PathBuf>) -> Self {
        self.denied.push(path.into());
        self
    }

    pub fn network(mut self, allow: bool) -> Self {
        self.allow_network = allow;
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn build(self) -> SandboxPolicy {
        SandboxPolicy {
            allowed_paths: self.allowed,
            denied_paths: self.denied,
            allow_network: self.allow_network,
            env_overrides: self.env,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_produces_correct_policy() {
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read_write("/workspace")
            .deny("/home/user/.ssh")
            .network(true)
            .env("FOO", "bar")
            .build();

        assert_eq!(policy.allowed_paths.len(), 2);
        assert_eq!(policy.allowed_paths[0].mode, AccessMode::ReadOnly);
        assert_eq!(policy.allowed_paths[1].mode, AccessMode::ReadWrite);
        assert_eq!(policy.denied_paths.len(), 1);
        assert!(policy.allow_network);
        assert_eq!(policy.env_overrides.get("FOO").unwrap(), "bar");
    }
}
