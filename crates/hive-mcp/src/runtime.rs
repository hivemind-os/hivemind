//! Runtime detection and resolution for MCP server commands.
//!
//! Maps MCP server commands to the runtime they require and checks whether
//! that runtime is available on the system (via PATH) or through a managed
//! runtime (hive-node-env, hive-python-env).

use std::path::PathBuf;

/// The runtime category required by an MCP server command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum McpRuntime {
    /// `npx`, `npm`, `node` commands — requires Node.js.
    Node,
    /// `uvx`, `uv`, `python`, `python3` commands — requires Python/uv.
    Python,
    /// `docker` commands — requires Docker Engine.
    Docker,
    /// `dotnet` commands — requires .NET SDK.
    Dotnet,
    /// Anything else — try PATH lookup.
    Unknown,
}

impl McpRuntime {
    /// Human-readable name for display.
    pub fn display_name(&self) -> &'static str {
        match self {
            McpRuntime::Node => "Node.js",
            McpRuntime::Python => "Python/uv",
            McpRuntime::Docker => "Docker",
            McpRuntime::Dotnet => ".NET SDK",
            McpRuntime::Unknown => "Unknown",
        }
    }

    /// Whether this runtime can be auto-managed by hivemind.
    pub fn is_manageable(&self) -> bool {
        matches!(self, McpRuntime::Node | McpRuntime::Python)
    }
}

impl std::fmt::Display for McpRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Result of checking whether a runtime is available.
#[derive(Debug, Clone)]
pub enum RuntimeStatus {
    /// Runtime is available on the system PATH.
    Available { path: PathBuf },
    /// Runtime is available through a managed installation.
    ManagedAvailable { path: PathBuf },
    /// Runtime is not installed but can be auto-installed by hivemind.
    Manageable { runtime: McpRuntime },
    /// Runtime is not installed and cannot be auto-managed.
    NotInstalled { install_hint: String },
}

impl RuntimeStatus {
    /// Whether the runtime is available (system or managed).
    pub fn is_available(&self) -> bool {
        matches!(self, RuntimeStatus::Available { .. } | RuntimeStatus::ManagedAvailable { .. })
    }

    /// Whether the runtime can be auto-installed.
    pub fn can_auto_install(&self) -> bool {
        matches!(self, RuntimeStatus::Manageable { .. })
    }
}

/// Detect what runtime an MCP server command requires based on the program name.
pub fn detect_runtime(command: &str) -> McpRuntime {
    // Extract just the program name (handle paths like /usr/bin/node).
    let program =
        std::path::Path::new(command).file_name().and_then(|n| n.to_str()).unwrap_or(command);

    // Strip .exe/.cmd/.bat extensions on Windows for matching.
    let program = program
        .strip_suffix(".exe")
        .or_else(|| program.strip_suffix(".cmd"))
        .or_else(|| program.strip_suffix(".bat"))
        .unwrap_or(program);

    match program {
        "npx" | "npm" | "node" | "corepack" => McpRuntime::Node,
        "uvx" | "uv" | "python" | "python3" | "pip" | "pip3" => McpRuntime::Python,
        "docker" | "docker-compose" => McpRuntime::Docker,
        "dotnet" => McpRuntime::Dotnet,
        _ => McpRuntime::Unknown,
    }
}

/// Check if a runtime is available on the system PATH.
///
/// Returns `Some(path)` if the binary is found, `None` otherwise.
pub fn find_on_path(binary_name: &str) -> Option<PathBuf> {
    which_binary(binary_name)
}

/// Get installation guidance for a runtime that isn't installed.
pub fn install_hint(runtime: McpRuntime) -> &'static str {
    match runtime {
        McpRuntime::Node => {
            "Node.js is not installed. HiveMind OS can install a managed Node.js runtime automatically, \
             or you can install it from https://nodejs.org/"
        }
        McpRuntime::Python => {
            "Python/uv is not installed. HiveMind OS can install a managed Python runtime automatically, \
             or you can install Python from https://python.org/"
        }
        McpRuntime::Docker => {
            "Docker is not installed. Please install Docker Desktop from https://docker.com/products/docker-desktop \
             or Docker Engine from https://docs.docker.com/engine/install/"
        }
        McpRuntime::Dotnet => {
            "The .NET SDK is not installed. Please install it from https://dotnet.microsoft.com/download"
        }
        McpRuntime::Unknown => {
            "The required program was not found on your PATH."
        }
    }
}

/// The primary binary to look for when checking if a runtime is on the system PATH.
fn runtime_probe_binary(runtime: McpRuntime) -> &'static str {
    match runtime {
        McpRuntime::Node => "node",
        McpRuntime::Python => "python3",
        McpRuntime::Docker => "docker",
        McpRuntime::Dotnet => "dotnet",
        McpRuntime::Unknown => "",
    }
}

/// Check runtime availability considering both system PATH and managed runtimes.
///
/// - `node_env_ready`: whether the managed Node.js environment is ready.
/// - `python_env_ready`: whether the managed Python environment is ready.
pub fn check_runtime(
    runtime: McpRuntime,
    node_env_ready: bool,
    python_env_ready: bool,
) -> RuntimeStatus {
    // First check managed runtimes (they take precedence when ready).
    match runtime {
        McpRuntime::Node if node_env_ready => {
            return RuntimeStatus::ManagedAvailable { path: PathBuf::from("(managed)") };
        }
        McpRuntime::Python if python_env_ready => {
            return RuntimeStatus::ManagedAvailable { path: PathBuf::from("(managed)") };
        }
        _ => {}
    }

    // Check system PATH.
    let probe = runtime_probe_binary(runtime);
    if !probe.is_empty() {
        if let Some(path) = find_on_path(probe) {
            return RuntimeStatus::Available { path };
        }
    }

    // For Unknown runtime, check the actual command on PATH.
    if runtime == McpRuntime::Unknown {
        // Caller should handle Unknown by checking the actual command binary
        // directly via find_on_path. Return NotInstalled as default.
        return RuntimeStatus::NotInstalled { install_hint: install_hint(runtime).to_string() };
    }

    // Not found — check if we can auto-install.
    if runtime.is_manageable() {
        RuntimeStatus::Manageable { runtime }
    } else {
        RuntimeStatus::NotInstalled { install_hint: install_hint(runtime).to_string() }
    }
}

/// Look up a binary on the system PATH.
fn which_binary(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var("PATH").ok()?;
    find_in_path_var(name, &path_var)
}

/// Resolve a command name to its full path using the given PATH string.
///
/// On Windows, `Command::new("npx").env("PATH", child_path).spawn()` resolves
/// the executable using the **parent** process's PATH, not the child's.  Call
/// this before `Command::new` to resolve bare command names against the
/// effective child PATH so the spawn finds the right binary.
///
/// Returns `None` if the command already contains a path separator (i.e. it's
/// already a path) or if it isn't found on the given PATH.
pub fn resolve_command_in_path(command: &str, path_var: &str) -> Option<PathBuf> {
    // Skip if the command is already an absolute/relative path.
    if command.contains('/') || command.contains('\\') || command.contains(':') {
        return None;
    }
    find_in_path_var(command, path_var)
}

/// Search for `name` in the directories listed in `path_var`.
fn find_in_path_var(name: &str, path_var: &str) -> Option<PathBuf> {
    for dir in std::env::split_paths(path_var) {
        // On Windows, check executable extensions FIRST.  Node.js ships
        // both `npx` (a Unix shell script) and `npx.cmd` (the Windows
        // batch wrapper).  The extensionless file is not a valid Win32
        // binary, so we must prefer .exe/.cmd/.bat.
        if cfg!(target_os = "windows") {
            for ext in &[".exe", ".cmd", ".bat"] {
                let with_ext = dir.join(format!("{name}{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
        }
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_node_commands() {
        assert_eq!(detect_runtime("npx"), McpRuntime::Node);
        assert_eq!(detect_runtime("npm"), McpRuntime::Node);
        assert_eq!(detect_runtime("node"), McpRuntime::Node);
        assert_eq!(detect_runtime("corepack"), McpRuntime::Node);
    }

    #[test]
    fn detect_python_commands() {
        assert_eq!(detect_runtime("uvx"), McpRuntime::Python);
        assert_eq!(detect_runtime("uv"), McpRuntime::Python);
        assert_eq!(detect_runtime("python"), McpRuntime::Python);
        assert_eq!(detect_runtime("python3"), McpRuntime::Python);
        assert_eq!(detect_runtime("pip"), McpRuntime::Python);
    }

    #[test]
    fn detect_docker_commands() {
        assert_eq!(detect_runtime("docker"), McpRuntime::Docker);
        assert_eq!(detect_runtime("docker-compose"), McpRuntime::Docker);
    }

    #[test]
    fn detect_dotnet_commands() {
        assert_eq!(detect_runtime("dotnet"), McpRuntime::Dotnet);
    }

    #[test]
    fn detect_unknown_commands() {
        assert_eq!(detect_runtime("my-custom-server"), McpRuntime::Unknown);
        assert_eq!(detect_runtime("some-binary"), McpRuntime::Unknown);
    }

    #[test]
    fn detect_with_path_prefix() {
        assert_eq!(detect_runtime("/usr/bin/node"), McpRuntime::Node);
        assert_eq!(detect_runtime("/usr/local/bin/npx"), McpRuntime::Node);
    }

    #[test]
    fn detect_with_windows_extension() {
        assert_eq!(detect_runtime("node.exe"), McpRuntime::Node);
        assert_eq!(detect_runtime("npx.cmd"), McpRuntime::Node);
        assert_eq!(detect_runtime("python.exe"), McpRuntime::Python);
    }

    #[test]
    fn node_is_manageable() {
        assert!(McpRuntime::Node.is_manageable());
        assert!(McpRuntime::Python.is_manageable());
        assert!(!McpRuntime::Docker.is_manageable());
        assert!(!McpRuntime::Dotnet.is_manageable());
        assert!(!McpRuntime::Unknown.is_manageable());
    }

    #[test]
    fn check_runtime_managed_node_ready() {
        let status = check_runtime(McpRuntime::Node, true, false);
        assert!(status.is_available());
        assert!(!status.can_auto_install());
    }

    #[test]
    fn check_runtime_managed_python_ready() {
        let status = check_runtime(McpRuntime::Python, false, true);
        assert!(status.is_available());
    }

    #[test]
    fn check_runtime_docker_not_installed() {
        // Docker is almost certainly not in a CI PATH with exact name match
        // but we can't guarantee — just check the logic doesn't panic.
        let status = check_runtime(McpRuntime::Docker, false, false);
        // Either Available (if docker is installed) or NotInstalled.
        match status {
            RuntimeStatus::Available { .. } | RuntimeStatus::NotInstalled { .. } => {}
            _ => panic!("unexpected status for Docker: {status:?}"),
        }
    }

    #[test]
    fn check_runtime_node_manageable_when_not_ready() {
        // If node is not on PATH and managed is not ready, should be Manageable.
        // This test may pass differently depending on whether node is on PATH.
        let status = check_runtime(McpRuntime::Node, false, false);
        match status {
            RuntimeStatus::Available { .. } | RuntimeStatus::Manageable { .. } => {}
            _ => panic!("expected Available or Manageable, got {status:?}"),
        }
    }

    #[test]
    fn install_hints_are_non_empty() {
        assert!(!install_hint(McpRuntime::Node).is_empty());
        assert!(!install_hint(McpRuntime::Python).is_empty());
        assert!(!install_hint(McpRuntime::Docker).is_empty());
        assert!(!install_hint(McpRuntime::Dotnet).is_empty());
        assert!(!install_hint(McpRuntime::Unknown).is_empty());
    }

    #[test]
    fn resolve_command_skips_paths() {
        // Commands that already contain path separators should return None.
        assert!(resolve_command_in_path("/usr/bin/node", "/usr/bin").is_none());
        assert!(resolve_command_in_path("C:\\node\\npx.cmd", "C:\\node").is_none());
        assert!(resolve_command_in_path("./my-server", "/tmp").is_none());
    }

    #[test]
    fn resolve_command_finds_nothing_in_empty_path() {
        assert!(resolve_command_in_path("npx", "").is_none());
    }
}
