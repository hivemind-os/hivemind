use crate::config::load_config;
use anyhow::{bail, Context, Result};
use hive_contracts::HiveMindConfig;
use reqwest::blocking::Client;
use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

pub use hive_contracts::DaemonStatus;

pub fn daemon_url(explicit: Option<&str>) -> Result<String> {
    if let Some(explicit) = explicit {
        return Ok(explicit.to_string());
    }

    if let Ok(from_env) = env::var("HIVEMIND_DAEMON_URL") {
        return Ok(from_env);
    }

    // Try reading the address discovery file written by the daemon on
    // startup.  This is the primary way to find a running daemon when
    // dynamic port allocation (port 0) is in use.
    if let Ok(paths) = crate::config::discover_paths() {
        let addr_file = paths.run_dir.join("daemon.addr");
        if let Ok(addr) = std::fs::read_to_string(&addr_file) {
            let addr = addr.trim();
            if !addr.is_empty() {
                return Ok(format!("http://{addr}"));
            }
        }
    }

    match load_config() {
        Ok(config) => Ok(config.base_url()),
        Err(_) => Ok(HiveMindConfig::default().base_url()),
    }
}

pub fn daemon_status(base_url: &str) -> Result<DaemonStatus> {
    http_client()?
        .get(format!("{base_url}/api/v1/daemon/status"))
        .send()
        .with_context(|| format!("failed to reach {base_url}"))?
        .error_for_status()
        .with_context(|| format!("daemon at {base_url} returned an error"))?
        .json::<DaemonStatus>()
        .context("failed to parse daemon status response")
}

pub fn daemon_stop(base_url: &str) -> Result<()> {
    let mut req = http_client()?.post(format!("{base_url}/api/v1/daemon/shutdown"));

    // Attach daemon auth token when available so the shutdown request
    // passes the Bearer-token middleware.
    if let Some(token) = crate::daemon_token::load_direct() {
        req = req.header("Authorization", format!("Bearer {token}"));
    }

    req.send()
        .with_context(|| format!("failed to reach {base_url}"))?
        .error_for_status()
        .with_context(|| format!("daemon at {base_url} rejected the shutdown request"))?;
    Ok(())
}

pub fn daemon_start(base_url: &str, explicit_bin: Option<&Path>) -> Result<bool> {
    if daemon_status(base_url).is_ok() {
        return Ok(false);
    }

    let daemon_bin = resolve_daemon_binary(explicit_bin)?;
    let mut command = Command::new(&daemon_bin);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());

    // On Windows, prevent the child process from opening a visible console window.
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child =
        command.spawn().with_context(|| format!("failed to start {}", daemon_bin.display()))?;

    // Detach the child process so it continues running after we exit.
    // We don't need the handle; the daemon will be reaped by init/systemd.
    std::mem::drop(child.stdin.take());
    std::mem::drop(child.stdout.take());
    std::mem::drop(child.stderr.take());

    for _ in 0..50 {
        if daemon_status(base_url).is_ok() {
            return Ok(true);
        }
        thread::sleep(Duration::from_millis(200));
    }

    bail!("hive-daemon did not become ready in time at {base_url}");
}

pub fn resolve_daemon_binary(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }

    if let Some(path) = env::var_os("HIVEMIND_DAEMON_BIN").map(PathBuf::from) {
        return Ok(path);
    }

    let exe_name = format!("hive-daemon{}", env::consts::EXE_SUFFIX);
    let current_exe = env::current_exe().context("failed to determine the current executable")?;
    let sibling = current_exe
        .parent()
        .map(Path::to_path_buf)
        .context("failed to determine the current executable directory")?
        .join(&exe_name);

    if sibling.exists() {
        return Ok(sibling);
    }

    // On macOS, Tauri bundles resources in Contents/Resources/ inside the .app.
    // The main exe lives at Contents/MacOS/<app>, so we walk up to Contents/
    // and look inside Resources/ for the daemon binary.
    #[cfg(target_os = "macos")]
    {
        if let Some(resources_candidate) = current_exe
            .parent() // Contents/MacOS/
            .and_then(|macos_dir| macos_dir.parent()) // Contents/
            .map(|contents| contents.join("Resources").join(&exe_name))
        {
            if resources_candidate.exists() {
                return Ok(resources_candidate);
            }
        }
    }

    if let Some(repo_root) = find_repo_root(&current_exe) {
        let debug_candidate = repo_root.join("target").join("debug").join(&exe_name);
        if debug_candidate.exists() {
            return Ok(debug_candidate);
        }

        let release_candidate = repo_root.join("target").join("release").join(&exe_name);
        if release_candidate.exists() {
            return Ok(release_candidate);
        }
    }

    // Check well-known system installation paths.
    // The macOS/Linux PKG installer places the daemon binary in /usr/local/bin.
    #[cfg(unix)]
    {
        let system_path = Path::new("/usr/local/bin").join(&exe_name);
        if system_path.exists() {
            return Ok(system_path);
        }
    }

    bail!(
        "could not locate hive-daemon binary via HIVEMIND_DAEMON_BIN, sibling path, or repo target directory"
    )
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    for ancestor in start.ancestors() {
        if ancestor.join("Cargo.toml").exists()
            && ancestor.join("crates").join("hive-daemon").exists()
        {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

fn http_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .context("failed to construct http client")
}

/// Result of a macOS TCC access request.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AppleAccessResult {
    /// "granted", "denied", or "error"
    pub status: String,
    /// Human-readable detail (daemon output or error message).
    pub detail: String,
}

/// Run the daemon binary with `--request-calendar-access` and/or
/// `--request-contacts-access` to trigger macOS TCC permission prompts.
///
/// In production (built .app bundle), the HiveMind OS desktop app is the
/// "responsible" process and has the calendar/contacts entitlements, so
/// macOS shows the TCC prompt.
///
/// In dev mode (VS Code terminal, etc.) the responsible process typically
/// lacks entitlements, so TCC silently denies.  The caller should check
/// `status == "denied"` and open System Settings as a fallback.
pub fn request_apple_access(calendar: bool, contacts: bool) -> Result<AppleAccessResult> {
    let daemon_bin = resolve_daemon_binary(None)?;

    let mut cmd = Command::new(&daemon_bin);
    if calendar {
        cmd.arg("--request-calendar-access");
    }
    if contacts {
        cmd.arg("--request-contacts-access");
    }

    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .with_context(|| format!("failed to run {}", daemon_bin.display()))?;

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let combined = format!("{}{}", stdout, stderr);
    let detail = combined.trim().to_string();

    let code = output.status.code().unwrap_or(1);
    let status = match code {
        0 => "granted".to_string(),
        2 => "denied".to_string(),
        _ => "error".to_string(),
    };

    Ok(AppleAccessResult { status, detail })
}
