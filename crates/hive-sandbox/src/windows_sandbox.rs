//! Windows sandboxing via ACL deny rules with automatic cleanup.
//!
//! Strategy (mirrors the macOS seatbelt deny-list approach):
//!
//! 1. Deny the user-profile directory (`%USERPROFILE%`) so the sandboxed
//!    process cannot read arbitrary user data.
//! 2. Re-allow specific subdirectories that the policy permits (workspace,
//!    hivemind home, temp dirs, etc.) by leaving them untouched — they inherit
//!    the original permissive ACL.
//! 3. Deny explicitly listed sensitive paths (`.ssh`, `.aws`, …).
//! 4. Optionally block network access by poisoning proxy environment
//!    variables and adding a Windows Firewall rule (best-effort, requires
//!    admin for the firewall rule).
//!
//! All ACL modifications are wrapped in a PowerShell `try/finally` block
//! that saves the original ACLs before modification and restores them when
//! the child process exits — even if it crashes or is terminated.
//!
//! Limitations:
//! - If PowerShell itself is killed with `taskkill /F`, the `finally` block
//!   won't run. This is an inherent Windows limitation; on next sandbox run
//!   the stale ACLs would be overwritten with fresh saves anyway.
//! - Network restriction via firewall rule requires admin and blocks by
//!   executable path (affects all instances of that executable). The proxy
//!   poisoning fallback only covers HTTP clients that respect proxy env vars.

use crate::policy::SandboxPolicy;
use crate::{SandboxError, SandboxedCommand};
use std::io::Write;

/// Generate a PowerShell script that sandboxes the command via ACL deny
/// rules with automatic cleanup in a `try/finally` block.
fn generate_wrapper_script(
    command: &str,
    policy: &SandboxPolicy,
    shell_program: Option<&str>,
    shell_flag: Option<&str>,
) -> String {
    let mut script = String::with_capacity(4096);

    script.push_str("# hive-sandbox restricted execution wrapper\n");
    script.push_str("$ErrorActionPreference = 'Continue'\n\n");

    // ── Helpers ─────────────────────────────────────────────────────
    // A hashtable that stores original ACLs keyed by path so we can
    // restore them in the finally block.
    script.push_str(
        r#"$savedAcls = @{}
$identity = [System.Security.Principal.WindowsIdentity]::GetCurrent().Name

function Save-And-Deny($path, $access) {
  if (-not (Test-Path $path)) { return }
  try {
    $savedAcls[$path] = Get-Acl $path
    $acl = Get-Acl $path
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule(
      $identity, $access, 'ContainerInherit,ObjectInherit', 'None', 'Deny')
    $acl.AddAccessRule($rule)
    Set-Acl $path $acl
  } catch {
    # Non-fatal: some paths may not allow ACL changes
  }
}

function Restore-AllAcls {
  foreach ($entry in $savedAcls.GetEnumerator()) {
    try { Set-Acl $entry.Key $entry.Value } catch {}
  }
}

"#,
    );

    // ── Build the allowed-paths set (lowercase for comparison) ──────
    let allowed_lc: Vec<String> =
        policy.allowed_paths.iter().map(|a| a.path.to_string_lossy().to_lowercase()).collect();

    script.push_str("$allowedPaths = @(\n");
    for p in &allowed_lc {
        script.push_str(&format!("  '{}'\n", p.replace('\'', "''")));
    }
    script.push_str(")\n\n");

    // Helper function to check if a path overlaps with an allowed path.
    script.push_str(
        r#"function Test-Allowed($dirPath) {
  $dp = $dirPath.ToLower().TrimEnd('\')
  foreach ($ap in $allowedPaths) {
    $a = $ap.TrimEnd('\')
    if ($dp -eq $a -or $a.StartsWith("$dp\") -or $dp.StartsWith("$a\")) {
      return $true
    }
  }
  return $false
}

"#,
    );

    // ── try/finally wrapper ─────────────────────────────────────────
    script.push_str("try {\n\n");

    // 1. Deny children of %USERPROFILE% that are not in the allowed set.
    //    This mirrors the macOS approach of denying $HOME then re-allowing
    //    specific subpaths. We deny individual children rather than the
    //    profile root itself so that allowed subpaths remain accessible
    //    without needing to break ACL inheritance.
    script.push_str(
        r#"  $profileDir = $env:USERPROFILE
  if ($profileDir -and (Test-Path $profileDir)) {
    Get-ChildItem -Path $profileDir -Force -ErrorAction SilentlyContinue | ForEach-Object {
      if (-not (Test-Allowed $_.FullName)) {
        Save-And-Deny $_.FullName 'Read'
      }
    }
  }

"#,
    );

    // 2. Explicitly denied paths (override any allows — e.g. .ssh inside
    //    an otherwise-allowed parent).
    for denied in &policy.denied_paths {
        let p = denied.to_string_lossy().replace('\'', "''");
        script.push_str(&format!("  Save-And-Deny '{}' 'FullControl'\n", p));
    }
    script.push('\n');

    // 3. Network restriction.
    if !policy.allow_network {
        // Best-effort: poison proxy env vars for the child process and
        // attempt a Windows Firewall outbound-block rule (requires admin).
        script.push_str(
            r#"  # Network restriction: poison proxy vars
  $env:HTTP_PROXY = 'http://127.0.0.1:1'
  $env:HTTPS_PROXY = 'http://127.0.0.1:1'
  $env:NO_PROXY = ''
  $env:http_proxy = 'http://127.0.0.1:1'
  $env:https_proxy = 'http://127.0.0.1:1'
  $env:no_proxy = ''

  # Attempt firewall rule (requires admin, non-fatal if it fails)
  $fwRuleName = "hive-sandbox-block-$PID"
  try {
    New-NetFirewallRule -DisplayName $fwRuleName -Direction Outbound -Action Block -Enabled True -ErrorAction Stop | Out-Null
  } catch {
    $fwRuleName = $null
  }

"#,
        );
    }

    // 4. Environment overrides.
    for (key, value) in &policy.env_overrides {
        let k = key.replace('\'', "''");
        let v = value.replace('\'', "''");
        script.push_str(&format!("  $env:{} = '{}'\n", k, v));
    }
    script.push('\n');

    // 5. Execute the command.
    let shell = shell_program.unwrap_or("cmd");
    let flag = shell_flag.unwrap_or("/c");
    script.push_str(&format!("  & '{}' {} \"{}\"\n", shell, flag, command.replace('"', "\\\"")));
    script.push_str("  $cmdExit = $LASTEXITCODE\n\n");

    // ── finally: restore ACLs and clean up firewall rule ────────────
    script.push_str("} finally {\n");
    script.push_str("  Restore-AllAcls\n");

    if !policy.allow_network {
        script.push_str(
            r#"  if ($fwRuleName) {
    try { Remove-NetFirewallRule -DisplayName $fwRuleName -ErrorAction Stop } catch {}
  }
"#,
        );
    }

    script.push_str("}\n\n");
    script.push_str("exit $cmdExit\n");

    script
}

#[allow(dead_code)]
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
    // Check if PowerShell is available
    let ps_available = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        std::process::Command::new("powershell")
            .arg("-Command")
            .arg("$true")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    };

    if !ps_available {
        return Err(SandboxError::Unavailable("PowerShell not available".into()));
    }

    let script = generate_wrapper_script(command, policy, shell_program, shell_flag);

    let mut tmp = tempfile::Builder::new().suffix(".ps1").tempfile()?;
    tmp.write_all(script.as_bytes())?;
    tmp.flush()?;
    let temp_path = tmp.into_temp_path();

    let script_path = temp_path.to_string_lossy().to_string();

    Ok(SandboxedCommand::Wrapped {
        program: "powershell".to_string(),
        args: vec![
            "-ExecutionPolicy".to_string(),
            "Bypass".to_string(),
            "-File".to_string(),
            script_path,
        ],
        _temp_files: vec![temp_path],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn wrapper_script_includes_deny_acls_for_denied_paths() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![
                PathBuf::from(r"C:\Users\test\.ssh"),
                PathBuf::from(r"C:\Users\test\.aws"),
            ],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        assert!(script.contains(r"C:\Users\test\.ssh"));
        assert!(script.contains(r"C:\Users\test\.aws"));
        assert!(script.contains("Save-And-Deny"));
        assert!(script.contains("echo hello"));
    }

    #[test]
    fn wrapper_script_denies_profile_children() {
        use crate::policy::{AccessMode, AllowedPath};
        let policy = SandboxPolicy {
            allowed_paths: vec![AllowedPath {
                path: PathBuf::from(r"C:\workspace"),
                mode: AccessMode::ReadWrite,
            }],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        // Should enumerate USERPROFILE children and deny non-allowed ones
        assert!(script.contains("$profileDir"));
        assert!(script.contains("Get-ChildItem"));
        assert!(script.contains("Test-Allowed"));
        assert!(script.contains("echo hello"));
    }

    #[test]
    fn wrapper_script_has_cleanup() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![PathBuf::from(r"C:\Users\test\.ssh")],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        // Must have try/finally with ACL restoration
        assert!(script.contains("try {"));
        assert!(script.contains("} finally {"));
        assert!(script.contains("Restore-AllAcls"));
        assert!(script.contains("$savedAcls"));
    }

    #[test]
    fn wrapper_script_restricts_network_when_disabled() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: false,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        // Proxy poisoning
        assert!(script.contains("HTTP_PROXY"));
        assert!(script.contains("127.0.0.1:1"));
        // Firewall rule attempt
        assert!(script.contains("New-NetFirewallRule"));
        assert!(script.contains("hive-sandbox-block"));
        // Cleanup
        assert!(script.contains("Remove-NetFirewallRule"));
    }

    #[test]
    fn wrapper_script_allows_network_when_enabled() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        assert!(!script.contains("HTTP_PROXY"));
        assert!(!script.contains("New-NetFirewallRule"));
    }

    #[test]
    fn wrapper_script_applies_env_overrides() {
        let mut env = std::collections::HashMap::new();
        env.insert("MY_VAR".to_string(), "my_value".to_string());
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: env,
        };
        let script = generate_wrapper_script("echo hello", &policy, None, None);
        assert!(script.contains("$env:MY_VAR = 'my_value'"));
    }

    #[test]
    fn wrapper_script_runs_command() {
        let policy = SandboxPolicy {
            allowed_paths: vec![],
            denied_paths: vec![],
            allow_network: true,
            env_overrides: Default::default(),
        };
        let script = generate_wrapper_script("dir /b", &policy, None, None);
        assert!(script.contains("dir /b"));
        assert!(script.contains("exit $cmdExit"));
    }
}
