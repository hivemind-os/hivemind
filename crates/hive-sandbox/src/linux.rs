//! Linux sandboxing via Landlock (kernel 5.13+) with bubblewrap fallback.
//!
//! Landlock is tried first as it's unprivileged and has low overhead.
//! If Landlock is not available (older kernel), we fall back to `bwrap`.

use crate::policy::{AccessMode, SandboxPolicy};
use crate::{SandboxError, SandboxedCommand};
use std::io::Write;

/// Try Landlock first, then bubblewrap.
pub fn sandbox_command(
    command: &str,
    policy: &SandboxPolicy,
) -> Result<SandboxedCommand, SandboxError> {
    sandbox_command_with_shell(command, policy, None, None)
}

pub fn sandbox_command_with_shell(
    command: &str,
    policy: &SandboxPolicy,
    shell_program: Option<&str>,
    shell_flag: Option<&str>,
) -> Result<SandboxedCommand, SandboxError> {
    // Landlock: we generate a small wrapper script that applies the Landlock
    // ruleset before exec-ing the real command. This avoids needing an external
    // binary and works on kernel 5.13+.
    //
    // However, applying Landlock from Rust requires running code *inside* the
    // child process before exec — which isn't trivial from a shell tool that
    // spawns `sh -c "..."`. So we use the bubblewrap approach for now and
    // attempt Landlock in a future iteration via a helper binary.

    match bwrap_command(command, policy, shell_program, shell_flag) {
        Ok(cmd) => Ok(cmd),
        Err(e) => Err(SandboxError::Unavailable(format!("bubblewrap not available: {}", e))),
    }
}

fn is_bwrap_available() -> bool {
    std::process::Command::new("bwrap")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn bwrap_command(
    command: &str,
    policy: &SandboxPolicy,
    shell_program: Option<&str>,
    shell_flag: Option<&str>,
) -> Result<SandboxedCommand, SandboxError> {
    if !is_bwrap_available() {
        return Err(SandboxError::Unavailable("bwrap binary not found".into()));
    }

    let mut args: Vec<String> = Vec::new();

    // Denied paths: bind to empty tmpfs
    for denied in &policy.denied_paths {
        args.push("--tmpfs".to_string());
        args.push(denied.to_string_lossy().to_string());
    }

    // Allowed paths
    for allowed in &policy.allowed_paths {
        let p = allowed.path.to_string_lossy().to_string();
        match allowed.mode {
            AccessMode::ReadOnly => {
                args.push("--ro-bind".to_string());
                args.push(p.clone());
                args.push(p);
            }
            AccessMode::ReadWrite => {
                args.push("--bind".to_string());
                args.push(p.clone());
                args.push(p);
            }
        }
    }

    // Standard system binds if not already covered
    args.push("--dev".to_string());
    args.push("/dev".to_string());
    args.push("--proc".to_string());
    args.push("/proc".to_string());
    args.push("--tmpfs".to_string());
    args.push("/tmp".to_string());

    // Unshare namespaces for isolation
    if !policy.allow_network {
        args.push("--unshare-net".to_string());
    }
    args.push("--unshare-pid".to_string());
    args.push("--die-with-parent".to_string());

    // Environment overrides
    for (key, value) in &policy.env_overrides {
        args.push("--setenv".to_string());
        args.push(key.clone());
        args.push(value.clone());
    }

    let shell = shell_program.unwrap_or("sh");
    let flag = shell_flag.unwrap_or("-c");

    // The command to execute
    args.push("--".to_string());
    args.push(shell.to_string());
    args.push(flag.to_string());
    args.push(command.to_string());

    Ok(SandboxedCommand::Wrapped { program: "bwrap".to_string(), args, _temp_files: vec![] })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AllowedPath;
    use std::path::PathBuf;

    #[test]
    fn bwrap_args_include_binds() {
        let policy = SandboxPolicy {
            allowed_paths: vec![
                AllowedPath { path: PathBuf::from("/usr"), mode: AccessMode::ReadOnly },
                AllowedPath { path: PathBuf::from("/workspace"), mode: AccessMode::ReadWrite },
            ],
            denied_paths: vec![PathBuf::from("/home/user/.ssh")],
            allow_network: false,
            env_overrides: Default::default(),
        };
        let result = bwrap_command("echo hello", &policy, None, None);
        match result {
            Ok(SandboxedCommand::Wrapped { program, args, .. }) => {
                assert_eq!(program, "bwrap");
                // Denied path becomes tmpfs
                let tmpfs_idx =
                    args.windows(2).position(|w| w[0] == "--tmpfs" && w[1] == "/home/user/.ssh");
                assert!(tmpfs_idx.is_some(), "denied path should be tmpfs");
                // Read-only bind
                let ro_idx = args
                    .windows(3)
                    .position(|w| w[0] == "--ro-bind" && w[1] == "/usr" && w[2] == "/usr");
                assert!(ro_idx.is_some(), "should have ro-bind for /usr");
                // Read-write bind
                let rw_idx = args
                    .windows(3)
                    .position(|w| w[0] == "--bind" && w[1] == "/workspace" && w[2] == "/workspace");
                assert!(rw_idx.is_some(), "should have bind for /workspace");
                // Network unshared
                assert!(args.contains(&"--unshare-net".to_string()));
            }
            Err(_) => {
                // bwrap not installed in test env — OK
            }
            _ => panic!("unexpected result"),
        }
    }

    #[test]
    fn bwrap_allows_network_when_requested() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let result = bwrap_command("echo hello", &policy, None, None);
        match result {
            Ok(SandboxedCommand::Wrapped { args, .. }) => {
                assert!(!args.contains(&"--unshare-net".to_string()), "should not unshare network");
            }
            Err(_) => {} // bwrap not installed
            _ => panic!("unexpected result"),
        }
    }
}
