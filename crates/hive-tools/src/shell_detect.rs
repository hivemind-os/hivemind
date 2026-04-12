use hive_contracts::{DetectedShells, ShellInfo, ShellKind};
use std::path::PathBuf;
use std::process::Command;

/// Detect available shells on the current system.
///
/// Probes the filesystem and PATH for known shells, retrieves version
/// information where possible, and selects a sensible default.
pub fn detect_shells() -> DetectedShells {
    let shells;

    #[cfg(not(target_os = "windows"))]
    {
        shells = detect_unix_shells();
    }

    #[cfg(target_os = "windows")]
    {
        shells = detect_windows_shells();
    }

    if shells.is_empty() {
        return DetectedShells::default();
    }

    let default_shell = pick_default(&shells);

    DetectedShells { shells, default_shell }
}

/// On Unix: read /etc/shells and also probe PATH for known shells.
#[cfg(not(target_os = "windows"))]
fn detect_unix_shells() -> Vec<ShellInfo> {
    use std::collections::HashSet;

    let known_shells = ["sh", "bash", "zsh", "fish", "nu", "pwsh"];
    let mut seen_paths = HashSet::new();
    let mut shells = Vec::new();

    // Parse /etc/shells for registered shells
    if let Ok(contents) = std::fs::read_to_string("/etc/shells") {
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let path = PathBuf::from(line);
            if path.exists() && seen_paths.insert(path.clone()) {
                let kind = kind_from_path(&path);
                let version = get_shell_version(&path, &kind);
                shells.push(ShellInfo { kind, path, version });
            }
        }
    }

    // Also probe PATH for known shells not already found
    for name in &known_shells {
        if let Some(path) = which_shell(name) {
            if seen_paths.insert(path.clone()) {
                let kind = ShellKind::from_name(name);
                let version = get_shell_version(&path, &kind);
                shells.push(ShellInfo { kind, path, version });
            }
        }
    }

    shells
}

/// On Windows: check for cmd, powershell, and pwsh.
#[cfg(target_os = "windows")]
fn detect_windows_shells() -> Vec<ShellInfo> {
    let mut shells = Vec::new();

    // cmd.exe is always available on Windows
    let cmd_path = PathBuf::from(r"C:\Windows\System32\cmd.exe");
    if cmd_path.exists() {
        shells.push(ShellInfo { kind: ShellKind::Cmd, path: cmd_path, version: None });
    }

    // Windows PowerShell (5.x)
    let ps_path = PathBuf::from(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe");
    if ps_path.exists() {
        let version = get_shell_version(&ps_path, &ShellKind::PowerShell);
        shells.push(ShellInfo { kind: ShellKind::PowerShell, path: ps_path, version });
    }

    // PowerShell Core (pwsh) - check PATH
    if let Some(path) = which_shell("pwsh") {
        let version = get_shell_version(&path, &ShellKind::Pwsh);
        shells.push(ShellInfo { kind: ShellKind::Pwsh, path, version });
    }

    // Also check for bash (e.g., Git Bash, WSL)
    if let Some(path) = which_shell("bash") {
        let version = get_shell_version(&path, &ShellKind::Bash);
        shells.push(ShellInfo { kind: ShellKind::Bash, path, version });
    }

    shells
}

/// Resolve a shell name to its full path using `command -v` (Unix) or `where` (Windows).
fn which_shell(name: &str) -> Option<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    {
        Command::new("sh")
            .args(["-c", &format!("command -v {}", name)])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| {
                let path = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if path.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(path))
                }
            })
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("where").arg(name).output().ok().filter(|o| o.status.success()).and_then(|o| {
            let output = String::from_utf8_lossy(&o.stdout);
            output.lines().next().map(|line| PathBuf::from(line.trim()))
        })
    }
}

/// Infer `ShellKind` from a shell's file path.
#[allow(dead_code, clippy::ptr_arg)]
fn kind_from_path(path: &PathBuf) -> ShellKind {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();

    // Strip version suffixes like "bash3.2" or "zsh5.9"
    let name = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');

    ShellKind::from_name(name)
}

/// Attempt to get the version string for a shell.
fn get_shell_version(path: &PathBuf, kind: &ShellKind) -> Option<String> {
    let output = match kind {
        ShellKind::Cmd => return None,
        ShellKind::PowerShell | ShellKind::Pwsh => Command::new(path)
            .args(["-NoProfile", "-Command", "$PSVersionTable.PSVersion.ToString()"])
            .output()
            .ok()?,
        ShellKind::Fish => Command::new(path).args(["--version"]).output().ok()?,
        ShellKind::Nushell => Command::new(path).args(["--version"]).output().ok()?,
        _ => Command::new(path).args(["--version"]).output().ok()?,
    };

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(parse_version_string(&raw, kind))
}

/// Extract a clean version number from raw --version output.
fn parse_version_string(raw: &str, kind: &ShellKind) -> String {
    // Take only the first line
    let first_line = raw.lines().next().unwrap_or(raw).trim();

    match kind {
        // "GNU bash, version 5.2.26(1)-release" -> "5.2.26"
        ShellKind::Bash => first_line
            .split("version ")
            .nth(1)
            .and_then(|v| v.split('(').next())
            .unwrap_or(first_line)
            .to_string(),
        // "zsh 5.9 (x86_64-apple-darwinXX)" -> "5.9"
        ShellKind::Zsh => first_line.split_whitespace().nth(1).unwrap_or(first_line).to_string(),
        // "fish, version 3.7.0" -> "3.7.0"
        ShellKind::Fish => first_line.split("version ").nth(1).unwrap_or(first_line).to_string(),
        // "0.92.0" (nu --version)
        ShellKind::Nushell => first_line.to_string(),
        // PowerShell version table output
        ShellKind::PowerShell | ShellKind::Pwsh => first_line.to_string(),
        _ => first_line.to_string(),
    }
}

/// Pick the best default shell from the detected list.
fn pick_default(shells: &[ShellInfo]) -> ShellKind {
    if cfg!(target_os = "windows") {
        // On Windows: prefer cmd
        for kind in &[ShellKind::Cmd, ShellKind::Pwsh, ShellKind::PowerShell] {
            if shells.iter().any(|s| &s.kind == kind) {
                return kind.clone();
            }
        }
    } else {
        // On Unix: prefer bash > zsh > sh
        for kind in &[ShellKind::Bash, ShellKind::Zsh, ShellKind::Sh] {
            if shells.iter().any(|s| &s.kind == kind) {
                return kind.clone();
            }
        }
    }
    shells.first().map(|s| s.kind.clone()).unwrap_or(ShellKind::Sh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_shells_returns_nonempty() {
        let detected = detect_shells();
        assert!(!detected.shells.is_empty(), "Should detect at least one shell");
        assert!(
            detected.default_shell_info().is_some(),
            "Default shell should be in the detected list"
        );
    }

    #[test]
    fn test_kind_from_path() {
        assert_eq!(kind_from_path(&PathBuf::from("/bin/bash")), ShellKind::Bash);
        assert_eq!(kind_from_path(&PathBuf::from("/bin/zsh")), ShellKind::Zsh);
        assert_eq!(kind_from_path(&PathBuf::from("/usr/bin/fish")), ShellKind::Fish);
        assert_eq!(kind_from_path(&PathBuf::from("/bin/sh")), ShellKind::Sh);
    }

    #[test]
    fn test_parse_version_bash() {
        let raw = "GNU bash, version 5.2.26(1)-release";
        assert_eq!(parse_version_string(raw, &ShellKind::Bash), "5.2.26");
    }

    #[test]
    fn test_parse_version_zsh() {
        let raw = "zsh 5.9 (x86_64-apple-darwin23.0)";
        assert_eq!(parse_version_string(raw, &ShellKind::Zsh), "5.9");
    }

    #[test]
    fn test_parse_version_fish() {
        let raw = "fish, version 3.7.0";
        assert_eq!(parse_version_string(raw, &ShellKind::Fish), "3.7.0");
    }

    #[test]
    fn test_description_summary() {
        let detected = DetectedShells {
            shells: vec![
                ShellInfo {
                    kind: ShellKind::Bash,
                    path: PathBuf::from("/bin/bash"),
                    version: Some("5.2.26".to_string()),
                },
                ShellInfo {
                    kind: ShellKind::Zsh,
                    path: PathBuf::from("/bin/zsh"),
                    version: Some("5.9".to_string()),
                },
            ],
            default_shell: ShellKind::Bash,
        };
        let summary = detected.description_summary();
        assert!(summary.contains("bash [5.2.26] (default)"));
        assert!(summary.contains("zsh [5.9]"));
        assert!(summary.contains("OS:"));
    }

    #[test]
    fn test_find_by_name() {
        let detected = DetectedShells {
            shells: vec![ShellInfo {
                kind: ShellKind::Bash,
                path: PathBuf::from("/bin/bash"),
                version: None,
            }],
            default_shell: ShellKind::Bash,
        };
        assert!(detected.find_by_name("bash").is_some());
        assert!(detected.find_by_name("BASH").is_some());
        assert!(detected.find_by_name("fish").is_none());
    }

    #[test]
    fn test_default_fallback() {
        let detected = DetectedShells::default();
        assert!(!detected.shells.is_empty());
        assert!(detected.default_shell_info().is_some());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_sh_is_always_detected() {
        let detected = detect_shells();
        assert!(detected.find(&ShellKind::Sh).is_some(), "sh should always be available on Unix");
    }
}
