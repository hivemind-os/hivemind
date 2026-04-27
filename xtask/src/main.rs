use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const HF_BASE: &str = "https://huggingface.co";
const BGE_REPO: &str = "BAAI/bge-small-en-v1.5";

/// (remote path relative to repo root, local filename)
const BGE_FILES: &[(&str, &str)] =
    &[("onnx/model.onnx", "model.onnx"), ("tokenizer.json", "tokenizer.json")];


fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cmd = args.get(1).map(|s| s.as_str()).unwrap_or("help");

    match cmd {
        "fetch-models" => fetch_models(),
        "build-installer" => build_installer(&args[2..]),
        "build-daemon" => build_daemon(&args[2..]),
        "run-daemon" => run_daemon(&args[2..]),
        "check-version" => check_version(),
        "bump-version" => bump_version(&args[2..]),
        _ => {
            eprintln!("Usage: cargo xtask <command>");
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  fetch-models                  Download embedding model files to vendor/");
            eprintln!("  build-installer [--target T]   Build installer for the current (or specified) platform");
            eprintln!(
                "  build-daemon [--release]       Build hive-daemon (with macOS codesign for TCC)"
            );
            eprintln!("  run-daemon [--release]         Build, codesign, and run hive-daemon");
            eprintln!("  check-version                  Assert Cargo.toml and tauri.conf.json versions match");
            eprintln!(
                "  bump-version <X.Y.Z>           Update version in Cargo.toml and tauri.conf.json"
            );
            eprintln!();
            eprintln!("Targets:");
            eprintln!("  macos-aarch64    macOS Apple Silicon PKG");
            eprintln!("  macos-x86_64     macOS Intel PKG");
            eprintln!("  windows-x64      Windows x64 NSIS installer");
            eprintln!("  windows-arm64    Windows ARM64 NSIS installer");
            std::process::exit(1);
        }
    }
}

fn project_root() -> PathBuf {
    let dir = std::env::var("CARGO_MANIFEST_DIR")
        .unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").to_string());
    Path::new(&dir).parent().expect("xtask must be inside project root").to_path_buf()
}

fn fetch_models() {
    let vendor = project_root().join("vendor").join("bge-small-en-v1.5");
    fs::create_dir_all(&vendor).expect("failed to create vendor directory");

    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("failed to build HTTP client");

    for &(remote, local) in BGE_FILES {
        let dest = vendor.join(local);
        if dest.exists() {
            println!("  ✓ {local} (already present)");
            continue;
        }

        let url = format!("{HF_BASE}/{BGE_REPO}/resolve/main/{remote}");
        println!("  ↓ downloading {local} ...");

        let resp =
            client.get(&url).send().unwrap_or_else(|e| panic!("failed to download {local}: {e}"));

        if !resp.status().is_success() {
            panic!("HTTP {} for {}", resp.status(), url);
        }

        let bytes =
            resp.bytes().unwrap_or_else(|e| panic!("failed to read response for {local}: {e}"));

        let mut f = fs::File::create(&dest)
            .unwrap_or_else(|e| panic!("failed to create {}: {e}", dest.display()));
        f.write_all(&bytes).unwrap_or_else(|e| panic!("failed to write {}: {e}", dest.display()));

        println!("  ✓ {} ({} bytes)", local, bytes.len());
    }

    println!();
    println!("Models ready at: {}", vendor.display());
}


fn build_installer(args: &[String]) {
    let target = parse_target_arg(args);
    let root = project_root();

    match target.as_str() {
        "macos-aarch64" => run_macos_build(&root, "aarch64"),
        "macos-x86_64" => run_macos_build(&root, "x86_64"),
        "windows-x64" => run_windows_build(&root, "x86_64-pc-windows-msvc"),
        "windows-arm64" => run_windows_build(&root, "aarch64-pc-windows-msvc"),
        other => {
            eprintln!("Unknown target: {other}");
            eprintln!("Valid targets: macos-aarch64, macos-x86_64, windows-x64, windows-arm64");
            std::process::exit(1);
        }
    }
}

fn parse_target_arg(args: &[String]) -> String {
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--target" {
            if let Some(val) = args.get(i + 1) {
                return val.clone();
            }
            eprintln!("--target requires a value");
            std::process::exit(1);
        }
        // Also support --target=value
        if let Some(val) = args[i].strip_prefix("--target=") {
            return val.to_string();
        }
        i += 1;
    }

    // Auto-detect platform
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match (os, arch) {
        ("macos", "aarch64") => "macos-aarch64".to_string(),
        ("macos", "x86_64") => "macos-x86_64".to_string(),
        ("windows", "x86_64") => "windows-x64".to_string(),
        ("windows", "aarch64") => "windows-arm64".to_string(),
        _ => {
            eprintln!("Cannot auto-detect target for {os}/{arch}. Use --target explicitly.");
            std::process::exit(1);
        }
    }
}

fn run_macos_build(root: &Path, arch: &str) {
    let script = root.join("scripts").join("macos").join("build-pkg.sh");
    if !script.exists() {
        eprintln!("macOS build script not found: {}", script.display());
        std::process::exit(1);
    }

    println!("==> Building macOS PKG for {arch}...");
    let status = std::process::Command::new("bash")
        .arg(&script)
        .arg(arch)
        .current_dir(root)
        .status()
        .expect("failed to run build-pkg.sh");

    if !status.success() {
        eprintln!("macOS build failed");
        std::process::exit(1);
    }
}

/// Download the VC++ Redistributable into `dest` if it doesn't already exist.
/// Returns `true` if the file was downloaded (so the caller can clean it up).
fn stage_vc_redist(dest: &Path, target: &str) -> bool {
    if dest.exists() {
        println!("  vc_redist.exe already present at {}", dest.display());
        return false;
    }

    let url = if target.starts_with("aarch64") {
        "https://aka.ms/vs/17/release/vc_redist.arm64.exe"
    } else {
        "https://aka.ms/vs/17/release/vc_redist.x64.exe"
    };

    println!("  Downloading VC++ Redistributable from {url}...");
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("failed to build HTTP client");

    let resp = client
        .get(url)
        .send()
        .unwrap_or_else(|e| panic!("failed to download vc_redist.exe: {e}"));
    if !resp.status().is_success() {
        panic!("HTTP {} downloading vc_redist.exe", resp.status());
    }
    let bytes = resp.bytes().unwrap_or_else(|e| panic!("failed to read vc_redist.exe: {e}"));
    if bytes.len() < 1_000_000 {
        panic!("vc_redist.exe is unexpectedly small ({} bytes)", bytes.len());
    }

    fs::write(dest, &bytes).unwrap_or_else(|e| panic!("failed to write vc_redist.exe: {e}"));
    println!(
        "  vc_redist.exe downloaded ({:.1} MB)",
        bytes.len() as f64 / 1_048_576.0
    );
    true
}

fn run_windows_build(root: &Path, target: &str) {
    println!("==> Building Windows NSIS installer for {target}...");

    // 1. Build daemon and CLI
    println!("  Building hive-daemon and hive-cli...");
    let mut cmd = std::process::Command::new("cargo");
    cmd.args([
        "build",
        "--release",
        "--target",
        target,
        "-p",
        "hive-daemon",
        "-p",
        "hive-cli",
        "-p",
        "hive-runtime-worker",
        "--features",
        "service-manager",
    ]);

    // llama.cpp ggml requires Clang for ARM targets (MSVC is not supported).
    // Switch to the Ninja generator with clang-cl when cross-compiling for ARM64.
    if target.starts_with("aarch64") {
        cmd.env("CMAKE_GENERATOR", "Ninja");
        let target_env = target.replace('-', "_");
        cmd.env(format!("CC_{target_env}"), "clang-cl");
        cmd.env(format!("CXX_{target_env}"), "clang-cl");
        cmd.env(format!("AR_{target_env}"), "llvm-lib");
    }

    let status = cmd.current_dir(root).status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("cargo build failed");
        std::process::exit(1);
    }

    let desktop_dir = root.join("apps").join("hivemind-desktop");

    // 2. Stage daemon and CLI binaries so Tauri can bundle them
    println!("  Staging daemon and CLI binaries for bundling...");
    let staging_dir = desktop_dir.join("src-tauri").join("bin");
    fs::create_dir_all(&staging_dir).expect("failed to create staging directory");

    let release_dir = root.join("target").join(target).join("release");
    for bin in &["hive-daemon.exe", "hive-cli.exe", "hive-runtime-worker.exe"] {
        let src = release_dir.join(bin);
        let dst = staging_dir.join(bin);
        fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("failed to copy {} to staging: {e}", src.display()));
    }

    // 3. Download VC++ Redistributable so the NSIS installer can bundle it
    let vc_redist_path = desktop_dir.join("src-tauri").join("vc_redist.exe");
    let vc_redist_downloaded = stage_vc_redist(&vc_redist_path, target);

    // 4. Build Tauri NSIS installer with the bundled binaries.
    //    The config override tells Tauri to include the staged binaries as
    //    resources, which places them alongside the main exe on Windows.
    //    Note: python-wasm is NOT bundled — it's downloaded on first daemon startup.
    println!("  Building Tauri installer...");
    // On Windows, npx is a .cmd shim that Command::new cannot resolve directly.
    let npx = if cfg!(target_os = "windows") { "npx.cmd" } else { "npx" };
    let status = std::process::Command::new(npx)
        .args([
            "tauri",
            "build",
            "--target",
            target,
            "--config",
            "src-tauri/tauri.windows-resources.conf.json",
            "--features",
            "service-manager",
        ])
        .current_dir(&desktop_dir)
        .status()
        .expect("failed to run tauri build");

    // Clean up staging directories
    let _ = fs::remove_dir_all(&staging_dir);
    if vc_redist_downloaded {
        let _ = fs::remove_file(&vc_redist_path);
    }

    if !status.success() {
        eprintln!("Tauri build failed");
        std::process::exit(1);
    }

    println!("==> Windows installer built successfully");
}

fn build_daemon(args: &[String]) {
    build_daemon_inner(args);
}

fn build_daemon_inner(args: &[String]) -> PathBuf {
    let root = project_root();
    let release = args.iter().any(|a| a == "--release");
    let profile_dir = if release { "release" } else { "debug" };

    println!("==> Building hive-daemon ({})...", profile_dir);
    let mut cmd = std::process::Command::new("cargo");
    cmd.args(["build", "-p", "hive-daemon", "-p", "hive-runtime-worker"]);
    if release {
        cmd.arg("--release");
    }
    // Forward remaining args (e.g. --features)
    for arg in args.iter().filter(|a| a.as_str() != "--release") {
        cmd.arg(arg);
    }
    let status = cmd.current_dir(&root).status().expect("failed to run cargo build");
    if !status.success() {
        eprintln!("cargo build failed");
        std::process::exit(1);
    }

    let binary = root.join("target").join(profile_dir).join("hive-daemon");

    // On macOS, re-sign the binary so the embedded Info.plist is bound to
    // the code signature.  Without this, macOS TCC silently denies
    // Calendar / Contacts access requests.
    //
    // CODESIGN_IDENTITY selects a named certificate (e.g. "HiveMind Dev")
    // that stays stable across rebuilds, preventing TCC / keychain churn.
    // Run scripts/macos/create-dev-cert.sh once to create the dev cert, then
    // add `export CODESIGN_IDENTITY="HiveMind Dev"` to your shell profile.
    // Falls back to ad-hoc signing (-) when the variable is unset.
    #[cfg(target_os = "macos")]
    if binary.exists() {
        let identity = std::env::var("CODESIGN_IDENTITY").unwrap_or_else(|_| "-".to_string());
        println!("==> Codesigning hive-daemon for macOS TCC (identity: {identity})...");
        let status = std::process::Command::new("codesign")
            .args(["--force", "--sign", &identity, "--identifier", "com.hivemind.daemon"])
            .arg(&binary)
            .status()
            .expect("failed to run codesign");
        if !status.success() {
            eprintln!("codesign failed");
            std::process::exit(1);
        }
        println!("==> Signed {} as com.hivemind.daemon", binary.display());
    }

    println!("==> hive-daemon build complete");
    binary
}

fn run_daemon(args: &[String]) {
    let binary = build_daemon_inner(args);

    println!("==> Running hive-daemon...");
    let status = std::process::Command::new(&binary).status().unwrap_or_else(|e| {
        eprintln!("failed to run {}: {e}", binary.display());
        std::process::exit(1);
    });
    std::process::exit(status.code().unwrap_or(1));
}

// ── Version management ────────────────────────────────────────────────────────

fn read_cargo_version(root: &Path) -> String {
    let text = fs::read_to_string(root.join("Cargo.toml")).expect("read Cargo.toml");
    // The workspace version lives under [workspace.package]; it is the first
    // `^version = "..."` line in the file.
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("version = \"") {
            if let Some(ver) = rest.strip_suffix('"') {
                return ver.to_string();
            }
        }
    }
    panic!("Could not find version in Cargo.toml");
}

fn read_tauri_version(root: &Path) -> String {
    let text = fs::read_to_string(root.join("apps/hivemind-desktop/src-tauri/tauri.conf.json"))
        .expect("read tauri.conf.json");
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("\"version\": \"") {
            let ver = rest.trim_end_matches(',').trim_end_matches('"');
            return ver.to_string();
        }
    }
    panic!("Could not find version in tauri.conf.json");
}

fn check_version() {
    let root = project_root();
    let cargo_ver = read_cargo_version(&root);
    let tauri_ver = read_tauri_version(&root);

    if cargo_ver == tauri_ver {
        println!("Version check passed: {cargo_ver}");
    } else {
        eprintln!("ERROR: version mismatch");
        eprintln!("  Cargo.toml          {cargo_ver}");
        eprintln!("  tauri.conf.json     {tauri_ver}");
        eprintln!();
        eprintln!("Fix with: cargo xtask bump-version <X.Y.Z>");
        std::process::exit(1);
    }
}

fn bump_version(args: &[String]) {
    let new_ver = match args.first() {
        Some(v) => v.clone(),
        None => {
            eprintln!("Usage: cargo xtask bump-version <X.Y.Z>");
            std::process::exit(1);
        }
    };

    // Basic semver sanity check
    let parts: Vec<&str> = new_ver.split('.').collect();
    if parts.len() != 3 || parts.iter().any(|p| p.parse::<u32>().is_err()) {
        eprintln!("Version must be X.Y.Z with numeric components, got: {new_ver}");
        std::process::exit(1);
    }

    let root = project_root();
    let old_cargo_ver = read_cargo_version(&root);
    let old_tauri_ver = read_tauri_version(&root);

    // Update Cargo.toml — replace the first occurrence (workspace.package version)
    let cargo_path = root.join("Cargo.toml");
    let cargo_text = fs::read_to_string(&cargo_path).expect("read Cargo.toml");
    let new_cargo = cargo_text.replacen(
        &format!("version = \"{old_cargo_ver}\""),
        &format!("version = \"{new_ver}\""),
        1,
    );
    fs::write(&cargo_path, new_cargo).expect("write Cargo.toml");

    // Update tauri.conf.json
    let tauri_path = root.join("apps/hivemind-desktop/src-tauri/tauri.conf.json");
    let tauri_text = fs::read_to_string(&tauri_path).expect("read tauri.conf.json");
    let new_tauri = tauri_text.replacen(
        &format!("\"version\": \"{old_tauri_ver}\""),
        &format!("\"version\": \"{new_ver}\""),
        1,
    );
    fs::write(&tauri_path, new_tauri).expect("write tauri.conf.json");

    println!("Bumped {old_cargo_ver} → {new_ver}");
    println!("  Cargo.toml");
    println!("  apps/hivemind-desktop/src-tauri/tauri.conf.json");
    println!();
    println!("Next steps:");
    println!("  git commit -am 'chore: bump version to {new_ver}'");
    println!("  git tag v{new_ver}");
}
