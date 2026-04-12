//! macOS sandbox via `sandbox-exec` and generated Seatbelt profiles.

use crate::policy::{AccessMode, SandboxPolicy};
use crate::{SandboxError, SandboxedCommand};
use std::io::Write;

/// Check whether `sandbox-exec` is available on this system.
///
/// We check only that the binary exists rather than spawning a test
/// process.  Spawning requires a free file descriptor for
/// `/dev/null`, and the daemon may have thousands of FDs open from
/// other subsystems, exhausting the soft limit.
fn is_available() -> bool {
    std::path::Path::new("/usr/bin/sandbox-exec").exists()
}

/// Generate a Seatbelt profile string from the policy.
///
/// Uses a **data-read deny-list** approach:
///
/// 1. `(deny default)` blocks all operations.
/// 2. Process, IPC, mach, and sysctl operations are allowed (required for
///    shell execution and dynamic linking).
/// 3. `(allow file-read-data)` is allowed globally because the macOS
///    dynamic linker (`dyld`) reads its shared cache from paths that
///    cannot be enumerated as subpath rules. Without this, processes
///    abort on startup.
/// 4. `(allow file-read-metadata)` is allowed globally so runtimes can
///    resolve real paths (stat/lstat/readlink).
/// 5. User-accessible data areas (`/Users`, `/Volumes`, `/home`) are
///    explicitly denied for `file-read-data`.
/// 6. Policy-allowed paths carve holes in the deny rules above, re-enabling
///    reads for the workspace, runtime directories, etc.
/// 7. Explicit denied-path overrides are applied last.
pub(crate) fn generate_seatbelt_profile(policy: &SandboxPolicy) -> String {
    let mut profile = String::with_capacity(2048);
    profile.push_str("(version 1)\n");
    profile.push_str("(deny default)\n\n");

    // Allow process, IPC, and mach operations (required for shell execution)
    profile.push_str("(allow process*)\n");
    profile.push_str("(allow signal)\n");
    profile.push_str("(allow sysctl-read)\n");
    profile.push_str("(allow mach*)\n");
    profile.push_str("(allow ipc*)\n\n");

    // Network
    if policy.allow_network {
        profile.push_str("(allow network*)\n\n");
    }

    // Global file-read-data: required for dyld shared cache and basic
    // process startup. Unlike the previous (allow file-read*), this only
    // grants data reads — not directory listings or xattr access.
    profile.push_str("(allow file-read-data)\n");
    // Global file-read-metadata: required for path resolution (stat/lstat).
    profile.push_str("(allow file-read-metadata)\n\n");

    // ── Deny user-accessible data areas ─────────────────────────────
    // The global file-read-data is needed for dyld, but we lock down
    // directories where user and sensitive data lives.  We also deny
    // writes so that only explicitly re-allowed paths (workspace, HOME
    // for shell tools) get write access.
    profile.push_str("; --- deny user data areas ---\n");
    profile.push_str("(deny file-read-data (subpath \"/Users\"))\n");
    profile.push_str("(deny file-read-data (subpath \"/Volumes\"))\n");
    profile.push_str("(deny file-read-data (subpath \"/home\"))\n");
    profile.push_str("(deny file-write* (subpath \"/Users\"))\n");
    profile.push_str("(deny file-write* (subpath \"/Volumes\"))\n");
    profile.push_str("(deny file-write* (subpath \"/home\"))\n\n");

    // ── Re-allow paths from policy ──────────────────────────────────
    // More-specific subpath allows override the deny rules above.
    // Callers populate allowed_paths with system paths, workspace, temp
    // dir, runtime directories, hivemind home, etc.
    for allowed in &policy.allowed_paths {
        let escaped = escape_seatbelt(&allowed.path.to_string_lossy());
        profile.push_str(&format!("(allow file-read-data (subpath \"{}\"))\n", escaped));
        profile.push_str(&format!("(allow file-read-xattr (subpath \"{}\"))\n", escaped));
        if allowed.mode == AccessMode::ReadWrite {
            profile.push_str(&format!("(allow file-write* (subpath \"{}\"))\n", escaped));
        }
    }
    profile.push('\n');

    // Allow writing to /dev (stdout/stderr/tty)
    profile.push_str("(allow file-write* (subpath \"/dev\"))\n");
    profile.push_str("(allow file-write* (subpath \"/private/var/folders\"))\n");
    profile.push_str("(allow file-write* (subpath \"/var/folders\"))\n\n");

    // Denied paths: final overrides that block even allowed subpaths
    // (e.g. ~/.ssh inside an otherwise-allowed home subpath).
    for denied in &policy.denied_paths {
        let p = denied.to_string_lossy();
        profile.push_str(&format!(
            "(deny file-read-data (subpath \"{e}\"))\n(deny file-read-xattr (subpath \"{e}\"))\n(deny file-write* (subpath \"{e}\"))\n",
            e = escape_seatbelt(&p),
        ));
    }

    profile
}

/// Escape special characters for Seatbelt profile strings.
fn escape_seatbelt(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

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
    if !is_available() {
        return Err(SandboxError::Unavailable("sandbox-exec not available".into()));
    }

    let profile = generate_seatbelt_profile(policy);
    tracing::debug!(
        profile_len = profile.len(),
        allowed_paths = policy.allowed_paths.len(),
        denied_paths = policy.denied_paths.len(),
        allow_network = policy.allow_network,
        "generated seatbelt profile"
    );

    // Write profile to a temp file
    let mut tmp = tempfile::NamedTempFile::new()?;
    tmp.write_all(profile.as_bytes())?;
    tmp.flush()?;
    let temp_path = tmp.into_temp_path();

    let profile_path = temp_path.to_string_lossy().to_string();
    tracing::debug!(
        profile_path = %profile_path,
        command = %command,
        "created sandbox profile"
    );

    let shell = shell_program.unwrap_or("sh");
    let flag = shell_flag.unwrap_or("-c");

    Ok(SandboxedCommand::Wrapped {
        program: "sandbox-exec".to_string(),
        args: vec![
            "-f".to_string(),
            profile_path,
            shell.to_string(),
            flag.to_string(),
            command.to_string(),
        ],
        _temp_files: vec![temp_path],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_denies_by_default() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: false,
            env_overrides: Default::default(),
        };
        let profile = generate_seatbelt_profile(&policy);
        assert!(profile.contains("(deny default)"));
        assert!(!profile.contains("(allow network*)"));
        // Global file-read-data (for dyld) but NOT file-read*
        assert!(profile.contains("(allow file-read-data)\n"));
        assert!(profile.contains("(allow file-read-metadata)\n"));
        // User data areas are denied
        assert!(profile.contains(r#"(deny file-read-data (subpath "/Users"))"#));
        assert!(profile.contains(r#"(deny file-read-data (subpath "/Volumes"))"#));
        assert!(profile.contains(r#"(deny file-read-data (subpath "/home"))"#));
    }

    #[test]
    fn profile_allows_network_when_requested() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let profile = generate_seatbelt_profile(&policy);
        assert!(profile.contains("(allow network*)"));
    }

    #[test]
    fn profile_denies_network_when_not_requested() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: false,
            env_overrides: Default::default(),
        };
        let profile = generate_seatbelt_profile(&policy);
        assert!(!profile.contains("(allow network*)"));
    }

    #[test]
    fn profile_includes_allowed_paths() {
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read_write("/workspace")
            .deny("/home/user/.ssh")
            .network(false)
            .build();
        let profile = generate_seatbelt_profile(&policy);
        // Allowed paths have per-subpath data read + xattr rules
        assert!(profile.contains(r#"(allow file-read-data (subpath "/usr"))"#));
        assert!(profile.contains(r#"(allow file-read-xattr (subpath "/usr"))"#));
        assert!(profile.contains(r#"(allow file-read-data (subpath "/workspace"))"#));
        // Write access only for ReadWrite paths
        assert!(profile.contains(r#"(allow file-write* (subpath "/workspace"))"#));
        // ReadOnly path must NOT have write access
        assert!(!profile.contains(r#"(allow file-write* (subpath "/usr"))"#));
        // Denied paths use data+xattr+write deny
        assert!(profile.contains(r#"(deny file-read-data (subpath "/home/user/.ssh"))"#));
        assert!(profile.contains(r#"(deny file-write* (subpath "/home/user/.ssh"))"#));
        // No bare (allow file-read*) without subpath
        for line in profile.lines() {
            assert_ne!(
                line.trim(),
                "(allow file-read*)",
                "profile must NOT contain bare (allow file-read*)"
            );
        }
    }

    #[test]
    fn sandbox_command_produces_wrapped() {
        let policy =
            SandboxPolicy::builder().allow_read_write("/tmp/test-workspace").network(true).build();
        let result = sandbox_command("echo hello", &policy);
        match result {
            Ok(SandboxedCommand::Wrapped { program, args, .. }) => {
                assert_eq!(program, "sandbox-exec");
                assert_eq!(args[0], "-f");
                // args[1] is temp file path
                assert_eq!(args[2], "sh");
                assert_eq!(args[3], "-c");
                assert_eq!(args[4], "echo hello");
            }
            Ok(SandboxedCommand::Passthrough) => {
                // sandbox-exec might not be available in CI
            }
            Err(_) => {
                // Expected if sandbox-exec is not available
            }
        }
    }
}

/// Real integration tests that spawn sandboxed processes and verify
/// file-system and network restrictions are enforced.
#[cfg(test)]
mod integration_tests {
    use super::*;
    use std::process::Command;

    fn skip_if_no_sandbox() -> bool {
        !Command::new("sandbox-exec")
            .arg("-n")
            .arg("no-network")
            .arg("true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Helper: run a command inside the sandbox and return (stdout, stderr, success).
    fn run_sandboxed(policy: &SandboxPolicy, shell_cmd: &str) -> (String, String, bool) {
        let profile = generate_seatbelt_profile(policy);
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut tmp, profile.as_bytes()).unwrap();
        std::io::Write::flush(&mut tmp).unwrap();
        let profile_path = tmp.path().to_string_lossy().to_string();

        let output = Command::new("sandbox-exec")
            .args(["-f", &profile_path, "sh", "-c", shell_cmd])
            .output()
            .expect("failed to spawn sandbox-exec");

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        (stdout, stderr, output.status.success())
    }

    #[test]
    fn sandbox_blocks_reading_home_directory() {
        if skip_if_no_sandbox() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        // Only allow system paths — home is NOT in the allow list
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/etc")
            .allow_read("/Library")
            .allow_read("/System")
            .allow_read("/dev")
            .network(false)
            .build();

        let (stdout, stderr, _) = run_sandboxed(&policy, &format!("ls {}", home));
        assert!(
            stderr.contains("Operation not permitted") || stdout.is_empty(),
            "sandbox should block reading home dir, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_blocks_reading_var_log() {
        if skip_if_no_sandbox() {
            return;
        }
        // /var/log is NOT in the allow list
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .network(false)
            .build();

        let (stdout, stderr, _) = run_sandboxed(&policy, "cat /var/log/system.log 2>&1 | head -1");
        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stdout.is_empty(),
            "sandbox should block /var/log, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_allows_reading_allowed_workspace() {
        if skip_if_no_sandbox() {
            return;
        }
        // Allow /etc as a "workspace" stand-in (always exists, always readable)
        let policy = SandboxPolicy::builder()
            .allow_read("/etc")
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .network(false)
            .build();

        let (stdout, _stderr, success) = run_sandboxed(&policy, "cat /etc/hosts | head -1");
        assert!(success, "reading allowed path should succeed");
        assert!(!stdout.is_empty(), "should produce output from /etc/hosts");
    }

    #[test]
    fn sandbox_allows_reallowed_home_subpath() {
        if skip_if_no_sandbox() {
            return;
        }
        // Create a temp dir under home, allow it, and verify access
        let home = std::env::var("HOME").unwrap();
        let test_dir = format!("{}/.__hivemind_sandbox_test", home);
        std::fs::create_dir_all(&test_dir).ok();
        let test_file = format!("{}/probe.txt", test_dir);
        std::fs::write(&test_file, "sandbox-ok").ok();

        let policy = SandboxPolicy::builder()
            .allow_read(&test_dir)
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .network(false)
            .build();

        let (stdout, _stderr, success) = run_sandboxed(&policy, &format!("cat {}", test_file));
        // Clean up
        std::fs::remove_dir_all(&test_dir).ok();

        assert!(success, "reading allowed subpath should succeed");
        assert_eq!(stdout.trim(), "sandbox-ok");
    }

    #[test]
    fn sandbox_blocks_network_when_disabled() {
        if skip_if_no_sandbox() {
            return;
        }
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/sbin")
            .allow_read("/etc")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .allow_read("/opt/homebrew")
            .allow_read_write(std::env::temp_dir())
            .allow_read("/private/var/folders")
            .allow_read("/var/folders")
            .network(false)
            .build();

        // curl should fail or produce no output when network is blocked
        let (stdout, _stderr, _) =
            run_sandboxed(&policy, "curl -s --max-time 3 http://example.com 2>&1");
        assert!(
            stdout.is_empty() || stdout.contains("curl"),
            "network should be blocked, got: {:?}",
            stdout
        );
    }

    #[test]
    fn sandbox_allows_network_when_enabled() {
        if skip_if_no_sandbox() {
            return;
        }
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/sbin")
            .allow_read("/etc")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .allow_read("/opt/homebrew")
            .allow_read_write(std::env::temp_dir())
            .allow_read("/private/var/folders")
            .allow_read("/var/folders")
            .network(true)
            .build();

        let (stdout, _stderr, success) =
            run_sandboxed(&policy, "curl -s --max-time 5 http://example.com | head -1");
        assert!(success, "curl should succeed with network enabled");
        assert!(
            stdout.contains("html") || stdout.contains("HTML") || stdout.contains("Example"),
            "should get HTTP response, got: {:?}",
            stdout
        );
    }

    #[test]
    fn sandbox_blocks_other_users() {
        if skip_if_no_sandbox() {
            return;
        }
        // /Users is NOT in the allow list — should be blocked
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .network(false)
            .build();

        let (stdout, stderr, _) = run_sandboxed(&policy, "ls /Users 2>&1");
        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted"),
            "should block /Users, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_denied_paths_override_allowed() {
        if skip_if_no_sandbox() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        let test_dir = format!("{}/.__hivemind_sandbox_test2", home);
        let secret_dir = format!("{}/secret", test_dir);
        std::fs::create_dir_all(&secret_dir).ok();
        std::fs::write(format!("{}/data.txt", secret_dir), "top-secret").ok();
        std::fs::write(format!("{}/ok.txt", test_dir), "public").ok();

        let policy = SandboxPolicy::builder()
            .allow_read(&test_dir)
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .deny(&secret_dir)
            .network(false)
            .build();

        // Allowed parent works
        let (stdout, _, _) = run_sandboxed(&policy, &format!("cat {}/ok.txt", test_dir));
        assert_eq!(stdout.trim(), "public");

        // Denied subdir blocked
        let (stdout, stderr, _) = run_sandboxed(&policy, &format!("cat {}/data.txt", secret_dir));

        // Clean up
        std::fs::remove_dir_all(&test_dir).ok();

        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stdout.is_empty(),
            "denied subpath should be blocked, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_allows_home_runtimes_via_path_entries() {
        if skip_if_no_sandbox() {
            return;
        }
        // Build a policy that mimics what hive-mcp produces: system paths
        // plus any PATH entries under $HOME so runtimes from nvm/pyenv/etc
        // are accessible.
        let home = match std::env::var("HOME").ok() {
            Some(h) => h,
            None => return,
        };
        let home_path = std::path::Path::new(&home);

        let mut builder = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/sbin")
            .allow_read("/etc")
            .allow_read("/Library")
            .allow_read("/System")
            .allow_read("/opt/homebrew")
            .allow_read("/dev")
            .allow_read("/tmp")
            .allow_read("/private/tmp")
            .allow_read("/private/var/folders")
            .allow_read("/var/folders")
            .network(false);

        // Add PATH entries under HOME — exactly what hive-mcp does.
        // Also add the parent of bin dirs for runtime lib/ access.
        if let Some(path_var) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&path_var) {
                if dir.starts_with(home_path) {
                    builder = builder.allow_read(&dir);
                    // If the dir ends in /bin, also allow the parent (runtime root)
                    if dir.ends_with("bin") {
                        if let Some(parent) = dir.parent() {
                            if parent != home_path {
                                builder = builder.allow_read(parent);
                            }
                        }
                    }
                }
            }
        }

        let policy = builder.build();

        // `node --version` should work even if node is under ~/.nvm
        let (stdout, stderr, success) = run_sandboxed(&policy, "node --version");
        assert!(success, "node should work with PATH entries allowed, stderr={:?}", stderr);
        assert!(stdout.starts_with('v'), "should get node version, got: {:?}", stdout);
    }

    #[test]
    fn sandbox_blocks_reading_outside_allowed_paths() {
        if skip_if_no_sandbox() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        let allowed_dir = format!("{}/.__hivemind_sandbox_allowed", home);
        let blocked_dir = format!("{}/.__hivemind_sandbox_blocked", home);
        std::fs::create_dir_all(&allowed_dir).ok();
        std::fs::create_dir_all(&blocked_dir).ok();
        std::fs::write(format!("{}/ok.txt", allowed_dir), "allowed-content").ok();
        std::fs::write(format!("{}/secret.txt", blocked_dir), "blocked-content").ok();

        // Only allow the first directory
        let policy = SandboxPolicy::builder()
            .allow_read(&allowed_dir)
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .network(false)
            .build();

        // Allowed dir should work
        let (stdout, _, success) = run_sandboxed(&policy, &format!("cat {}/ok.txt", allowed_dir));
        assert!(success, "reading allowed dir should succeed");
        assert_eq!(stdout.trim(), "allowed-content");

        // Blocked dir should fail
        let (stdout, stderr, _) =
            run_sandboxed(&policy, &format!("cat {}/secret.txt", blocked_dir));

        // Clean up
        std::fs::remove_dir_all(&allowed_dir).ok();
        std::fs::remove_dir_all(&blocked_dir).ok();

        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stdout.is_empty(),
            "reading non-allowed dir should be blocked, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_blocks_reading_non_users_paths() {
        if skip_if_no_sandbox() {
            return;
        }
        // Create a temp file outside /Users (under /private/tmp)
        let test_dir = "/private/tmp/hivemind_sandbox_test_non_users";
        std::fs::create_dir_all(test_dir).ok();
        std::fs::write(format!("{}/secret.txt", test_dir), "outside-users").ok();

        // Policy does NOT include /private/tmp in allowed paths
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .network(false)
            .build();

        let (stdout, stderr, _) = run_sandboxed(&policy, &format!("cat {}/secret.txt", test_dir));

        // Clean up
        std::fs::remove_dir_all(test_dir).ok();

        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stdout.is_empty(),
            "reading non-allowed path outside /Users should be blocked, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    /// Simulates the shell/process tool sandbox: HOME is allowed for read
    /// (so build tool caches work), but sensitive sub-dirs are still denied.
    #[test]
    fn sandbox_allows_home_but_denies_sensitive_subdirs() {
        if skip_if_no_sandbox() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        let cache_dir = format!("{}/.___hivemind_sandbox_cache_test", home);
        let secret_dir = format!("{}/.___hivemind_sandbox_secret_test", home);
        std::fs::create_dir_all(&cache_dir).ok();
        std::fs::create_dir_all(&secret_dir).ok();
        std::fs::write(format!("{}/pkg.tar", cache_dir), "cached-package").ok();
        std::fs::write(format!("{}/key.pem", secret_dir), "private-key").ok();

        let policy = SandboxPolicy::builder()
            .allow_read(&home)
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .deny(&secret_dir)
            .network(false)
            .build();

        // Cache dir (under HOME) should be readable
        let (stdout, _, success) = run_sandboxed(&policy, &format!("cat {}/pkg.tar", cache_dir));
        assert!(success, "reading cache dir under HOME should succeed");
        assert_eq!(stdout.trim(), "cached-package");

        // Secret dir (denied) should be blocked
        let (stdout, stderr, _) = run_sandboxed(&policy, &format!("cat {}/key.pem", secret_dir));

        // Clean up
        std::fs::remove_dir_all(&cache_dir).ok();
        std::fs::remove_dir_all(&secret_dir).ok();

        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stdout.is_empty(),
            "denied secret dir should be blocked even with HOME allowed, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    /// Simulates the shell tool sandbox with HOME read-write: verifies that a
    /// sandboxed process can write to a cache dir under HOME, but cannot write
    /// to a denied sensitive sub-directory.
    #[test]
    fn sandbox_allows_home_write_but_denies_sensitive_dirs() {
        if skip_if_no_sandbox() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        let cache_dir = format!("{}/.___hivemind_sandbox_write_test", home);
        let secret_dir = format!("{}/.___hivemind_sandbox_write_secret", home);
        std::fs::create_dir_all(&cache_dir).ok();
        std::fs::create_dir_all(&secret_dir).ok();

        let policy = SandboxPolicy::builder()
            .allow_read_write(&home)
            .allow_read("/usr")
            .allow_read("/bin")
            .allow_read("/dev")
            .allow_read("/Library")
            .allow_read("/System")
            .deny(&secret_dir)
            .network(false)
            .build();

        // Writing to cache dir under HOME should work
        let write_cmd =
            format!("echo 'cached' > {}/test.txt && cat {}/test.txt", cache_dir, cache_dir);
        let (stdout, stderr, success) = run_sandboxed(&policy, &write_cmd);
        assert!(success, "writing to cache dir under HOME should succeed, stderr={:?}", stderr);
        assert_eq!(stdout.trim(), "cached");

        // Writing to denied dir should fail
        let (stdout, stderr, _) =
            run_sandboxed(&policy, &format!("echo 'secret' > {}/key.txt", secret_dir));

        // Clean up
        std::fs::remove_dir_all(&cache_dir).ok();
        std::fs::remove_dir_all(&secret_dir).ok();

        assert!(
            stderr.contains("Operation not permitted")
                || stdout.contains("Operation not permitted")
                || stderr.contains("Permission denied"),
            "writing to denied dir should fail, got stdout={:?} stderr={:?}",
            stdout,
            stderr
        );
    }

    #[test]
    fn sandbox_profile_has_no_global_file_read_allow() {
        let policy = SandboxPolicy::builder()
            .allow_read("/usr")
            .allow_read_write("/workspace")
            .allow_read("/tmp")
            .network(true)
            .build();
        let profile = generate_seatbelt_profile(&policy);

        // Must NOT contain bare (allow file-read*) — only file-read-data
        for line in profile.lines() {
            let trimmed = line.trim();
            assert_ne!(
                trimmed, "(allow file-read*)",
                "profile must NOT contain bare (allow file-read*) — found: {:?}",
                trimmed
            );
        }

        // Must have global file-read-data for dyld
        assert!(
            profile.contains("(allow file-read-data)\n"),
            "should have global file-read-data for dyld"
        );

        // Must deny user data areas (read)
        assert!(
            profile.contains(r#"(deny file-read-data (subpath "/Users"))"#),
            "should deny read /Users"
        );
        assert!(
            profile.contains(r#"(deny file-read-data (subpath "/Volumes"))"#),
            "should deny read /Volumes"
        );

        // Must deny user data areas (write)
        assert!(
            profile.contains(r#"(deny file-write* (subpath "/Users"))"#),
            "should deny write /Users"
        );
        assert!(
            profile.contains(r#"(deny file-write* (subpath "/Volumes"))"#),
            "should deny write /Volumes"
        );
        assert!(
            profile.contains(r#"(deny file-write* (subpath "/home"))"#),
            "should deny write /home"
        );

        // Per-path allow rules ARE present (using file-read-data, not file-read*)
        assert!(
            profile.contains(r#"(allow file-read-data (subpath "/usr"))"#),
            "should have per-path allow for /usr"
        );
        assert!(
            profile.contains(r#"(allow file-read-data (subpath "/workspace"))"#),
            "should have per-path allow for /workspace"
        );
        assert!(
            profile.contains(r#"(allow file-read-data (subpath "/tmp"))"#),
            "should have per-path allow for /tmp"
        );
    }
}
