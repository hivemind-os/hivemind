//! Daemon service management for per-user auto-start.
//!
//! Provides load/unload/status operations for the OS-level service that keeps
//! `hive-daemon` running across reboots:
//!
//! - **macOS**: `~/Library/LaunchAgents/com.hivemind.daemon.plist`
//! - **Windows**: `HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run` registry key
//!
//! All operations require the `service-manager` Cargo feature (disabled by
//! default).  When the feature is absent the public functions become no-ops,
//! so the daemon is never registered as an OS service — this is the default
//! for local dev builds.  Installer / release builds pass
//! `--features service-manager` to opt in.

use anyhow::Result;
use tracing::info;

/// Whether the daemon auto-start service is currently registered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceStatus {
    /// Service is registered and will auto-start.
    Loaded,
    /// Service is not registered.
    Unloaded,
    /// Could not determine status.
    Unknown,
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServiceStatus::Loaded => write!(f, "loaded (daemon will auto-start at login)"),
            ServiceStatus::Unloaded => write!(f, "unloaded (daemon will NOT auto-start)"),
            ServiceStatus::Unknown => write!(f, "unknown"),
        }
    }
}

//  When feature = "service-manager" is ENABLED

/// Check whether the daemon auto-start service is currently registered.
#[cfg(feature = "service-manager")]
pub fn service_status() -> ServiceStatus {
    #[cfg(target_os = "macos")]
    return macos::status();

    #[cfg(target_os = "windows")]
    return windows::status();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    ServiceStatus::Unknown
}

/// Unload the daemon auto-start service so it won't restart.
///
/// On macOS this calls `launchctl bootout`.
/// On Windows this removes the registry Run key.
#[cfg(feature = "service-manager")]
pub fn service_unload() -> Result<()> {
    #[cfg(target_os = "macos")]
    return macos::unload();

    #[cfg(target_os = "windows")]
    return windows::unload();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        info!("no service manager on this platform");
        Ok(())
    }
}

/// Load (register) the daemon auto-start service.
///
/// On macOS this writes the plist and calls `launchctl bootstrap`.
/// On Windows this sets the registry Run key.
#[cfg(feature = "service-manager")]
pub fn service_load() -> Result<()> {
    #[cfg(target_os = "macos")]
    return macos::load();

    #[cfg(target_os = "windows")]
    return windows::load();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        info!("no service manager on this platform");
        Ok(())
    }
}

//  When feature = "service-manager" is DISABLED ─

/// Always returns `Unknown` when service management is compiled out.
#[cfg(not(feature = "service-manager"))]
pub fn service_status() -> ServiceStatus {
    ServiceStatus::Unknown
}

/// No-op  service registration is compiled out.
#[cfg(not(feature = "service-manager"))]
pub fn service_unload() -> Result<()> {
    info!("service-manager feature disabled  skipping service unload");
    Ok(())
}

/// No-op  service registration is compiled out.
#[cfg(not(feature = "service-manager"))]
pub fn service_load() -> Result<()> {
    info!("service-manager feature disabled  skipping service registration");
    Ok(())
}

//  macOS LaunchAgent

#[cfg(all(feature = "service-manager", target_os = "macos"))]
mod macos {
    use super::*;
    use crate::{discover_paths, resolve_daemon_binary};
    use std::fs;
    use std::process::Command;

    const LABEL: &str = "com.hivemind.daemon";

    fn plist_path() -> Result<std::path::PathBuf> {
        let home =
            dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
        Ok(home.join("Library").join("LaunchAgents").join(format!("{LABEL}.plist")))
    }

    fn uid() -> u32 {
        unsafe { libc::getuid() }
    }

    fn domain_target() -> String {
        format!("gui/{}", uid())
    }

    fn service_target() -> String {
        format!("gui/{uid}/{LABEL}", uid = uid())
    }

    pub fn status() -> ServiceStatus {
        let Ok(output) = Command::new("launchctl")
            .args(["print", &service_target()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
        else {
            return ServiceStatus::Unknown;
        };
        if output.success() {
            ServiceStatus::Loaded
        } else {
            ServiceStatus::Unloaded
        }
    }

    pub fn unload() -> Result<()> {
        let plist = plist_path()?;
        if !plist.exists() {
            info!("no LaunchAgent plist found  nothing to unload");
            return Ok(());
        }

        let st = Command::new("launchctl")
            .args(["bootout", &domain_target(), &plist.to_string_lossy()])
            .status()?;

        if st.success() {
            info!("LaunchAgent unloaded");
        } else {
            // Already unloaded is fine.
            info!("launchctl bootout exited with {st} (may already be unloaded)");
        }

        Ok(())
    }

    pub fn load() -> Result<()> {
        let daemon_bin = resolve_daemon_binary(None)?;
        let paths = discover_paths()?;
        let log_dir = paths.hivemind_home.join("logs");
        fs::create_dir_all(&log_dir)?;

        let plist = plist_path()?;
        let plist_content = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{daemon_bin}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>StandardOutPath</key>
    <string>{log_dir}/launchd-daemon.out.log</string>
    <key>StandardErrorPath</key>
    <string>{log_dir}/launchd-daemon.err.log</string>
</dict>
</plist>"#,
            daemon_bin = daemon_bin.display(),
            log_dir = log_dir.display(),
        );

        // If plist already has the right content, just ensure it's loaded.
        if plist.exists() {
            let existing = fs::read_to_string(&plist).unwrap_or_default();
            if existing == plist_content {
                // Already correct  make sure it's actually loaded.
                if status() == ServiceStatus::Loaded {
                    info!("LaunchAgent already loaded with correct config");
                    return Ok(());
                }
                // Plist is correct but not loaded  bootstrap it.
            } else {
                // Plist changed  unload old one first.
                info!("updating LaunchAgent (daemon binary path changed)");
                let _ = unload();
            }
        }

        if let Some(parent) = plist.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&plist, &plist_content)?;

        let st = Command::new("launchctl")
            .args(["bootstrap", &domain_target(), &plist.to_string_lossy()])
            .status();

        match st {
            Ok(s) if s.success() => {
                info!("LaunchAgent loaded");
            }
            Ok(_) => {
                // bootstrap fails if already loaded  kickstart instead.
                let _ =
                    Command::new("launchctl").args(["kickstart", "-k", &service_target()]).status();
                info!("LaunchAgent kickstarted");
            }
            Err(e) => {
                anyhow::bail!("failed to run launchctl bootstrap: {e}");
            }
        }

        Ok(())
    }
}

//  Windows registry Run key ─

#[cfg(all(feature = "service-manager", target_os = "windows"))]
mod windows {
    use super::*;
    use crate::resolve_daemon_binary;
    use std::os::windows::process::CommandExt;
    use std::process::Command;

    const REG_KEY: &str = r"HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const VALUE_NAME: &str = "HiveMindDaemon";

    fn creation_flags() -> u32 {
        0x08000000 // CREATE_NO_WINDOW
    }

    pub fn status() -> ServiceStatus {
        let Ok(output) = Command::new("reg")
            .args(["query", REG_KEY, "/v", VALUE_NAME])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(creation_flags())
            .output()
        else {
            return ServiceStatus::Unknown;
        };
        if output.status.success() {
            ServiceStatus::Loaded
        } else {
            ServiceStatus::Unloaded
        }
    }

    pub fn unload() -> Result<()> {
        let st = Command::new("reg")
            .args(["delete", REG_KEY, "/v", VALUE_NAME, "/f"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(creation_flags())
            .status()?;

        if st.success() {
            info!("Windows auto-start registry key removed");
        } else {
            info!("registry key may not have existed (already unloaded)");
        }

        Ok(())
    }

    pub fn load() -> Result<()> {
        let daemon_bin = resolve_daemon_binary(None)?;
        let daemon_path_str = daemon_bin.to_string_lossy().to_string();

        // Check if already registered with correct path.
        if let Ok(output) = Command::new("reg")
            .args(["query", REG_KEY, "/v", VALUE_NAME])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .creation_flags(creation_flags())
            .output()
        {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                if stdout.contains(&daemon_path_str) {
                    info!("Windows auto-start already registered with correct path");
                    return Ok(());
                }
                info!("updating Windows auto-start (daemon binary path changed)");
            }
        }

        let st = Command::new("reg")
            .args(["add", REG_KEY, "/v", VALUE_NAME, "/t", "REG_SZ", "/d", &daemon_path_str, "/f"])
            .creation_flags(creation_flags())
            .status()?;

        if st.success() {
            info!("Windows auto-start registered");
        } else {
            anyhow::bail!("reg add exited with {st}");
        }

        Ok(())
    }
}
