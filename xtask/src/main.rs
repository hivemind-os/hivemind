use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const HF_BASE: &str = "https://huggingface.co";
const BGE_REPO: &str = "BAAI/bge-small-en-v1.5";

/// (remote path relative to repo root, local filename)
const BGE_FILES: &[(&str, &str)] =
    &[("onnx/model.onnx", "model.onnx"), ("tokenizer.json", "tokenizer.json")];

const PYTHON_WASM_URL: &str = "https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/python%2F3.12.0%2B20231211-040d5a6/python-3.12.0-wasi-sdk-20.0.tar.gz";
const PYTHON_WASM_ASSET: &str = "python-3.12.0-wasi-sdk-20.0.tar.gz";

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

/// Download and stage the CPython WASI runtime into `src-tauri/python-wasm/`
/// so Tauri can bundle it as a resource. Returns the staging path for cleanup.
fn stage_python_wasm(desktop_dir: &Path) -> PathBuf {
    let staging = desktop_dir.join("src-tauri").join("python-wasm");

    // Skip download if already staged (e.g. re-running after a Tauri failure)
    let wasm_bin = staging.join("bin").join("python.wasm");
    if wasm_bin.exists() {
        println!("  python.wasm already staged at {}", staging.display());
        return staging;
    }

    println!("  Downloading CPython WASI runtime...");
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .expect("failed to build HTTP client");

    let resp = client
        .get(PYTHON_WASM_URL)
        .send()
        .unwrap_or_else(|e| panic!("failed to download {PYTHON_WASM_ASSET}: {e}"));
    if !resp.status().is_success() {
        panic!("HTTP {} downloading {PYTHON_WASM_ASSET}", resp.status());
    }
    let tar_bytes = resp
        .bytes()
        .unwrap_or_else(|e| panic!("failed to read {PYTHON_WASM_ASSET}: {e}"));
    println!(
        "  Downloaded {} ({:.1} MB)",
        PYTHON_WASM_ASSET,
        tar_bytes.len() as f64 / 1_048_576.0
    );

    // Extract to a temp dir, then copy to the normalized layout
    let temp_dir = std::env::temp_dir().join(format!("hivemind-python-wasm-{}", std::process::id()));
    let _ = fs::remove_dir_all(&temp_dir);
    fs::create_dir_all(&temp_dir).expect("failed to create temp dir");

    let tar_path = temp_dir.join(PYTHON_WASM_ASSET);
    fs::write(&tar_path, &tar_bytes).expect("failed to write tarball");

    println!("  Extracting...");
    let status = std::process::Command::new("tar")
        .args(["-xzf", &tar_path.to_string_lossy()])
        .current_dir(&temp_dir)
        .status()
        .expect("failed to run tar");
    if !status.success() {
        panic!("tar extraction failed");
    }

    // Stage into normalized layout: python-wasm/{bin/python.wasm, lib/python3.12/, lib/python312.zip}
    let bin_dir = staging.join("bin");
    let lib_dir = staging.join("lib");
    fs::create_dir_all(&bin_dir).expect("failed to create python-wasm/bin");
    fs::create_dir_all(&lib_dir).expect("failed to create python-wasm/lib");

    // Find python*.wasm in extracted bin/
    let extracted_bin = temp_dir.join("bin");
    let wasm_file = fs::read_dir(&extracted_bin)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", extracted_bin.display()))
        .filter_map(|e| e.ok())
        .find(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("python") && name.ends_with(".wasm")
        })
        .unwrap_or_else(|| panic!("no python*.wasm found in archive"));
    fs::copy(wasm_file.path(), bin_dir.join("python.wasm"))
        .expect("failed to copy python.wasm");

    // Copy stdlib directory
    let stdlib_src = temp_dir.join("usr").join("local").join("lib").join("python3.12");
    if stdlib_src.exists() {
        copy_dir_recursive(&stdlib_src, &lib_dir.join("python3.12"));
    }

    // Copy zipped stdlib
    let zip_src = temp_dir.join("usr").join("local").join("lib").join("python312.zip");
    if zip_src.exists() {
        fs::copy(&zip_src, lib_dir.join("python312.zip")).expect("failed to copy python312.zip");
    }

    let _ = fs::remove_dir_all(&temp_dir);
    println!("  python.wasm staged at {}", staging.display());
    staging
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap_or_else(|e| panic!("mkdir {}: {e}", dst.display()));
    for entry in fs::read_dir(src).unwrap_or_else(|e| panic!("read {}: {e}", src.display())) {
        let entry = entry.expect("dir entry");
        let dest = dst.join(entry.file_name());
        if entry.file_type().expect("file_type").is_dir() {
            copy_dir_recursive(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), &dest)
                .unwrap_or_else(|e| panic!("copy {}: {e}", entry.path().display()));
        }
    }
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

    // 3. Stage python-wasm runtime so Tauri bundles it as a resource
    println!("  Staging python-wasm runtime...");
    let wasm_staging = stage_python_wasm(&desktop_dir);

    // 4. Build Tauri NSIS installer with the bundled binaries.
    //    The config override tells Tauri to include the staged binaries as
    //    resources, which places them alongside the main exe on Windows.
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
    let _ = fs::remove_dir_all(&wasm_staging);

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
