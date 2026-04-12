use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Known shell types that can be detected and selected.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ShellKind {
    Sh,
    Bash,
    Zsh,
    Fish,
    #[serde(rename = "powershell")]
    PowerShell,
    Cmd,
    Nushell,
    #[serde(rename = "pwsh")]
    Pwsh,
    Other(String),
}

impl ShellKind {
    /// The flag used to pass a command string to this shell.
    pub fn command_flag(&self) -> &str {
        match self {
            ShellKind::Sh
            | ShellKind::Bash
            | ShellKind::Zsh
            | ShellKind::Fish
            | ShellKind::Nushell => "-c",
            ShellKind::PowerShell | ShellKind::Pwsh => "-Command",
            ShellKind::Cmd => "/C",
            ShellKind::Other(_) => "-c",
        }
    }

    /// Parse a shell kind from a user-provided string (case-insensitive).
    pub fn from_name(name: &str) -> Self {
        match name.to_lowercase().as_str() {
            "sh" => ShellKind::Sh,
            "bash" => ShellKind::Bash,
            "zsh" => ShellKind::Zsh,
            "fish" => ShellKind::Fish,
            "powershell" | "powershell.exe" => ShellKind::PowerShell,
            "pwsh" | "pwsh.exe" => ShellKind::Pwsh,
            "cmd" | "cmd.exe" => ShellKind::Cmd,
            "nu" | "nushell" => ShellKind::Nushell,
            other => ShellKind::Other(other.to_string()),
        }
    }
}

impl fmt::Display for ShellKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShellKind::Sh => write!(f, "sh"),
            ShellKind::Bash => write!(f, "bash"),
            ShellKind::Zsh => write!(f, "zsh"),
            ShellKind::Fish => write!(f, "fish"),
            ShellKind::PowerShell => write!(f, "powershell"),
            ShellKind::Cmd => write!(f, "cmd"),
            ShellKind::Nushell => write!(f, "nushell"),
            ShellKind::Pwsh => write!(f, "pwsh"),
            ShellKind::Other(name) => write!(f, "{}", name),
        }
    }
}

/// Information about a detected shell on the system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellInfo {
    pub kind: ShellKind,
    pub path: PathBuf,
    pub version: Option<String>,
}

impl ShellInfo {
    /// The program name or path to invoke this shell.
    pub fn program(&self) -> &str {
        self.path.to_str().unwrap_or(match self.kind {
            ShellKind::Sh => "sh",
            ShellKind::Bash => "bash",
            ShellKind::Zsh => "zsh",
            ShellKind::Fish => "fish",
            ShellKind::PowerShell => "powershell",
            ShellKind::Cmd => "cmd",
            ShellKind::Nushell => "nu",
            ShellKind::Pwsh => "pwsh",
            ShellKind::Other(ref name) => name.as_str(),
        })
    }
}

/// The set of shells detected on the current system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedShells {
    pub shells: Vec<ShellInfo>,
    pub default_shell: ShellKind,
}

impl DetectedShells {
    /// Look up a shell by kind.
    pub fn find(&self, kind: &ShellKind) -> Option<&ShellInfo> {
        self.shells.iter().find(|s| &s.kind == kind)
    }

    /// Look up a shell by name string (case-insensitive).
    pub fn find_by_name(&self, name: &str) -> Option<&ShellInfo> {
        let kind = ShellKind::from_name(name);
        self.find(&kind)
    }

    /// Get the default shell info.
    pub fn default_shell_info(&self) -> Option<&ShellInfo> {
        self.find(&self.default_shell)
    }

    /// List available shell names for display.
    pub fn available_names(&self) -> Vec<String> {
        self.shells.iter().map(|s| s.kind.to_string()).collect()
    }

    /// Build a human-readable summary for tool descriptions.
    pub fn description_summary(&self) -> String {
        let os = std::env::consts::OS;
        let arch = std::env::consts::ARCH;
        let mut parts = vec![format!("OS: {} ({})", os, arch)];

        let shell_list: Vec<String> = self
            .shells
            .iter()
            .map(|s| {
                let default_marker = if s.kind == self.default_shell { " (default)" } else { "" };
                match &s.version {
                    Some(v) => format!("{} [{}]{}", s.kind, v, default_marker),
                    None => format!("{}{}", s.kind, default_marker),
                }
            })
            .collect();

        parts.push(format!("Available shells: {}", shell_list.join(", ")));
        parts.join(". ")
    }
}

impl Default for DetectedShells {
    fn default() -> Self {
        let default_shell =
            if cfg!(target_os = "windows") { ShellKind::Cmd } else { ShellKind::Sh };
        let path = if cfg!(target_os = "windows") {
            PathBuf::from("cmd.exe")
        } else {
            PathBuf::from("/bin/sh")
        };
        Self {
            shells: vec![ShellInfo { kind: default_shell.clone(), path, version: None }],
            default_shell,
        }
    }
}
