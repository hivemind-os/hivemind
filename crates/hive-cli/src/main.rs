use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hive_core::{
    config_to_yaml, daemon_start, daemon_status, daemon_stop, daemon_url, load_config,
    service_load, service_status, service_unload, validate_config_file,
};
use std::path::PathBuf;

/// The GitHub endpoint serving the Tauri updater manifest.
const UPDATE_MANIFEST_URL: &str =
    "https://github.com/hivemind-os/hivemind/releases/latest/download/latest.json";

#[derive(Debug, Parser)]
#[command(name = "hive")]
#[command(about = "HiveMind OS command line interface")]
struct Cli {
    #[command(subcommand)]
    command: TopLevelCommand,
}

#[derive(Debug, Subcommand)]
enum TopLevelCommand {
    Daemon {
        #[command(subcommand)]
        command: DaemonCommand,
    },
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Check for available updates and display download instructions.
    Update,
}

#[derive(Debug, Subcommand)]
enum DaemonCommand {
    Start {
        #[arg(long)]
        daemon_bin: Option<PathBuf>,
        #[arg(long)]
        url: Option<String>,
    },
    Status {
        #[arg(long)]
        url: Option<String>,
    },
    Stop {
        #[arg(long)]
        url: Option<String>,
        /// Also unload the auto-start service so the daemon stays stopped
        /// (prevents launchd/Windows from restarting it).
        #[arg(long)]
        no_restart: bool,
    },
    /// Register the daemon to auto-start at login.
    Load,
    /// Unregister the daemon auto-start (it won't restart after being stopped).
    Unload,
}

#[derive(Debug, Subcommand)]
enum ConfigCommand {
    Show,
    Validate {
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        TopLevelCommand::Daemon { command } => handle_daemon(command),
        TopLevelCommand::Config { command } => handle_config(command),
        TopLevelCommand::Update => handle_update(),
    }
}

fn handle_daemon(command: DaemonCommand) -> Result<()> {
    match command {
        DaemonCommand::Start { daemon_bin, url } => start_daemon_command(daemon_bin, url),
        DaemonCommand::Status { url } => status_daemon_command(url),
        DaemonCommand::Stop { url, no_restart } => stop_daemon_command(url, no_restart),
        DaemonCommand::Load => load_service_command(),
        DaemonCommand::Unload => unload_service_command(),
    }
}

fn handle_config(command: ConfigCommand) -> Result<()> {
    match command {
        ConfigCommand::Show => {
            let config = load_config().context("failed to load hivemind config")?;
            print!("{}", config_to_yaml(&config)?);
            Ok(())
        }
        ConfigCommand::Validate { path } => {
            match path {
                Some(path) => {
                    validate_config_file(&path)
                        .with_context(|| format!("config {} is invalid", path.display()))?;
                    println!("Config at {} is valid.", path.display());
                }
                None => {
                    load_config().context("hivemind config is invalid")?;
                    println!("Default HiveMind OS configuration is valid.");
                }
            }
            Ok(())
        }
    }
}

fn start_daemon_command(daemon_bin: Option<PathBuf>, url: Option<String>) -> Result<()> {
    let base_url = daemon_url(url.as_deref())?;
    let started = daemon_start(&base_url, daemon_bin.as_deref())?;

    if started {
        println!("HiveMind OS daemon started at {base_url}.");
    } else {
        println!("HiveMind OS daemon is already running at {base_url}.");
    }

    Ok(())
}

fn status_daemon_command(url: Option<String>) -> Result<()> {
    let base_url = daemon_url(url.as_deref())?;
    let status = daemon_status(&base_url)?;
    println!("HiveMind OS daemon is running.");
    println!("  PID: {}", status.pid);
    println!("  Version: {}", status.version);
    println!("  Platform: {}", status.platform);
    println!("  Bind: {}", status.bind);
    println!("  Uptime: {:.2}s", status.uptime_secs);
    Ok(())
}

fn stop_daemon_command(url: Option<String>, no_restart: bool) -> Result<()> {
    if no_restart {
        service_unload().context("failed to unload auto-start service")?;
        println!("Auto-start service unloaded.");
    }

    let base_url = daemon_url(url.as_deref())?;
    daemon_stop(&base_url)?;
    println!("HiveMind OS daemon shutdown requested.");

    if no_restart {
        println!("The daemon will NOT restart automatically. Use `hive daemon load` to re-enable.");
    }

    Ok(())
}

fn load_service_command() -> Result<()> {
    service_load().context("failed to load auto-start service")?;
    let status = service_status();
    println!("Auto-start service: {status}");
    Ok(())
}

fn unload_service_command() -> Result<()> {
    service_unload().context("failed to unload auto-start service")?;
    let status = service_status();
    println!("Auto-start service: {status}");
    println!("The daemon will NOT restart automatically. Use `hive daemon load` to re-enable.");
    Ok(())
}

// ── Update command ──────────────────────────────────────────────────

fn handle_update() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: {current}");
    println!("Checking for updates...");

    let manifest = fetch_update_manifest().context("failed to fetch update manifest")?;

    if manifest.version == current {
        println!("You are running the latest version.");
        return Ok(());
    }

    // Simple semver comparison: split on '.', compare numerically.
    if !is_newer(&manifest.version, current) {
        println!("You are running the latest version.");
        return Ok(());
    }

    println!("\nUpdate available: v{} → v{}", current, manifest.version);
    if !manifest.notes.is_empty() {
        println!("  {}", manifest.notes);
    }

    let platform_key = current_platform_key();
    if let Some(entry) = manifest.platforms.get(&platform_key) {
        println!("\nDownload URL:");
        println!("  {}", entry.url);
    } else {
        println!("\nNo pre-built update available for your platform ({platform_key}).");
        println!(
            "Visit https://github.com/hivemind-os/hivemind/releases/latest for all downloads."
        );
    }

    Ok(())
}

#[derive(serde::Deserialize)]
struct UpdateManifest {
    version: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    platforms: std::collections::HashMap<String, PlatformEntry>,
}

#[derive(serde::Deserialize)]
struct PlatformEntry {
    url: String,
}

fn fetch_update_manifest() -> Result<UpdateManifest> {
    let client =
        reqwest::blocking::Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
    let resp = client.get(UPDATE_MANIFEST_URL).send()?;
    let manifest: UpdateManifest = resp.json()?;
    Ok(manifest)
}

/// Return the Tauri-style platform key for the current host.
fn current_platform_key() -> String {
    let os = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") { "aarch64" } else { "x86_64" };
    format!("{os}-{arch}")
}

/// Return true if `new_ver` is strictly newer than `current` using simple
/// numeric comparison of dot-separated segments.
fn is_newer(new_ver: &str, current: &str) -> bool {
    let parse =
        |v: &str| -> Vec<u64> { v.split('.').map(|s| s.parse::<u64>().unwrap_or(0)).collect() };
    let new_parts = parse(new_ver);
    let cur_parts = parse(current);
    for i in 0..new_parts.len().max(cur_parts.len()) {
        let n = new_parts.get(i).copied().unwrap_or(0);
        let c = cur_parts.get(i).copied().unwrap_or(0);
        if n > c {
            return true;
        }
        if n < c {
            return false;
        }
    }
    false
}
