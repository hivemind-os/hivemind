//! WASM-sandboxed Python code execution for the CodeAct agent loop.
//!
//! This crate provides a [`CodeExecutor`] trait and a Wasmtime-based
//! implementation for running LLM-generated Python code in a true
//! WASM sandbox (memory limits, no filesystem beyond preopened dirs,
//! no network, no subprocess spawning).
//!
//! # Architecture
//!
//! ```text
//! CodeActStrategy (hive-loop)
//!     │
//!     ▼
//! SessionRegistry ── manages per-conversation WASM sessions
//!     │
//!     ▼
//! CodeExecutor trait
//!     │
//!     └── WasmExecutor  (Wasmtime + CPython WASI)
//! ```
//!
//! # Session Lifecycle
//!
//! Each conversation gets a persistent Python execution session. Variables,
//! imports, and state survive across code blocks within the same conversation.
//! Sessions are reaped after an idle timeout or when the conversation ends.

pub mod executor;
pub mod session;
pub mod tool_bridge;
pub mod wasm_executor;

pub use executor::{
    CodeExecutor, ExecutionResult, ExecutorConfig, ExecutorError, Language,
};
pub use session::{Session, SessionConfig, SessionRegistry, WasmRuntime};
pub use wasm_executor::WasmExecutor;
pub use tool_bridge::{
    BridgedToolInfo, CodeActToolMode, ExecutionOptions, ToolCallHandler,
    ToolCallRequest, ToolCallResponse, tool_id_to_python_name,
};

/// Resolved paths to the CPython WASI runtime.
#[derive(Debug, Clone)]
pub struct PythonWasmPaths {
    /// Path to the `python.wasm` binary.
    pub wasm_binary: std::path::PathBuf,
    /// Path to the Python standard library directory (e.g. `lib/python3.12`).
    pub stdlib_dir: std::path::PathBuf,
}

/// Discover the CPython WASI runtime from well-known locations.
///
/// Resolution order:
/// 1. `PYTHON_WASM_PATH` + `PYTHON_WASM_STDLIB` environment variables
/// 2. `{hivemind_home}/runtimes/python-wasm/bin/python.wasm` + `lib/python3.12`
/// 3. Sibling of current executable: `python-wasm/bin/python.wasm` + `lib/python3.12`
/// 4. macOS: `Contents/Resources/python-wasm/` inside the .app bundle
///
/// Returns `None` if no valid runtime is found.
///
/// **Validation**: In addition to checking `python.wasm` and the stdlib directory,
/// this also verifies that `python312.zip` (or the equivalent for the detected
/// version) exists in the stdlib parent. CPython WASI requires this zip archive
/// for the `encodings` module during `Py_Initialize()` — without it, CPython
/// crashes immediately with a fatal error before reading stdin.
pub fn resolve_python_wasm(hivemind_home: Option<&std::path::Path>) -> Option<PythonWasmPaths> {
    use std::path::PathBuf;

    // Helper: check that wasm binary, stdlib dir, AND the critical zip archive all exist.
    // CPython WASI hard-crashes during Py_Initialize() if the zip is missing.
    let check = |wasm: PathBuf, stdlib: PathBuf| -> Option<PythonWasmPaths> {
        if !wasm.exists() || !stdlib.exists() {
            return None;
        }

        // Derive the zip filename from the stdlib directory name.
        // e.g. "python3.12" → "python312.zip" (dots removed from version)
        let stdlib_name = stdlib.file_name()?.to_str()?;
        let zip_name = format!("{}.zip", stdlib_name.replace('.', ""));
        let zip_path = stdlib.parent()?.join(&zip_name);

        if !zip_path.exists() {
            tracing::warn!(
                zip = %zip_path.display(),
                stdlib = %stdlib.display(),
                "python.wasm found but critical stdlib zip archive is missing — \
                 CPython will crash during initialization without it"
            );
            return None;
        }

        Some(PythonWasmPaths {
            wasm_binary: wasm,
            stdlib_dir: stdlib,
        })
    };

    // 1. Environment variables (highest priority)
    if let (Ok(wasm), Ok(stdlib)) = (
        std::env::var("PYTHON_WASM_PATH"),
        std::env::var("PYTHON_WASM_STDLIB"),
    ) {
        if let Some(paths) = check(PathBuf::from(&wasm), PathBuf::from(&stdlib)) {
            tracing::debug!("python.wasm resolved from env vars");
            return Some(paths);
        }
    }

    // 2. HiveMind data directory
    if let Some(home) = hivemind_home {
        let runtime_dir = home.join("runtimes").join("python-wasm");
        let wasm = runtime_dir.join("bin").join("python.wasm");
        // Try multiple Python versions
        for version in &["python3.13", "python3.12", "python3.11"] {
            let stdlib = runtime_dir.join("lib").join(version);
            if let Some(paths) = check(wasm.clone(), stdlib) {
                tracing::debug!(dir = %runtime_dir.display(), "python.wasm resolved from hivemind home");
                return Some(paths);
            }
        }
    }

    // 3. Sibling of current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let runtime_dir = exe_dir.join("python-wasm");
            let wasm = runtime_dir.join("bin").join("python.wasm");
            for version in &["python3.13", "python3.12", "python3.11"] {
                let stdlib = runtime_dir.join("lib").join(version);
                if let Some(paths) = check(wasm.clone(), stdlib) {
                    tracing::debug!(dir = %runtime_dir.display(), "python.wasm resolved from exe sibling");
                    return Some(paths);
                }
            }

            // 4. macOS .app bundle: Contents/Resources/python-wasm/
            #[cfg(target_os = "macos")]
            {
                if let Some(resources) = exe_dir
                    .parent() // Contents/MacOS/ → Contents/
                    .map(|contents| contents.join("Resources").join("python-wasm"))
                {
                    let wasm = resources.join("bin").join("python.wasm");
                    for version in &["python3.13", "python3.12", "python3.11"] {
                        let stdlib = resources.join("lib").join(version);
                        if let Some(paths) = check(wasm.clone(), stdlib) {
                            tracing::debug!(dir = %resources.display(), "python.wasm resolved from macOS Resources");
                            return Some(paths);
                        }
                    }
                }
            }
        }
    }

    None
}

/// URL for the CPython WASI runtime tarball.
const PYTHON_WASM_URL: &str = "https://github.com/vmware-labs/webassembly-language-runtimes/releases/download/python%2F3.12.0%2B20231211-040d5a6/python-3.12.0-wasi-sdk-20.0.tar.gz";

/// Expected SHA-256 digest of the tarball (lowercase hex).
const PYTHON_WASM_SHA256: &str =
    "6c1cddbb69ae09e87eee2906bdc70539bff5f2969818a6f8457d4e6a6eb67d4d";

/// Ensure the CPython WASI runtime is available, downloading it if necessary.
///
/// First calls [`resolve_python_wasm`] to check existing installations.
/// If none is found, downloads the runtime from GitHub to
/// `{hivemind_home}/runtimes/python-wasm/` with atomic installation
/// (extract to temp dir, validate, rename into place).
///
/// This is a **blocking** function — safe to call before the tokio runtime
/// starts (e.g. during `AppState::new()`).
///
/// Returns `Err` if the download fails or the extracted layout is invalid.
/// Caller should degrade gracefully (CodeAct disabled) on error.
pub fn ensure_python_wasm(
    hivemind_home: &std::path::Path,
) -> Result<PythonWasmPaths, String> {
    use flate2::read::GzDecoder;
    use sha2::Digest;
    use tar::Archive;

    // Check if already installed anywhere
    if let Some(paths) = resolve_python_wasm(Some(hivemind_home)) {
        return Ok(paths);
    }

    tracing::info!("CPython WASI runtime not found — downloading...");

    let target_dir = hivemind_home.join("runtimes").join("python-wasm");

    // Download with timeout
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .map_err(|e| format!("failed to create HTTP client: {e}"))?;

    let resp = client
        .get(PYTHON_WASM_URL)
        .send()
        .map_err(|e| format!("failed to download CPython WASI runtime: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!(
            "failed to download CPython WASI runtime: HTTP {}",
            resp.status()
        ));
    }

    let bytes = resp
        .bytes()
        .map_err(|e| format!("failed to read response body: {e}"))?;

    tracing::info!(
        size_mb = format!("{:.1}", bytes.len() as f64 / 1_048_576.0),
        "CPython WASI runtime downloaded, verifying checksum..."
    );

    // Verify SHA-256
    let digest = sha2::Sha256::digest(&bytes);
    let actual_hash = hex::encode(digest);
    if actual_hash != PYTHON_WASM_SHA256 {
        return Err(format!(
            "CPython WASI runtime checksum mismatch: expected {}, got {}",
            PYTHON_WASM_SHA256, actual_hash
        ));
    }

    // Extract to a temp directory alongside the target (same filesystem for atomic rename)
    let runtimes_dir = hivemind_home.join("runtimes");
    std::fs::create_dir_all(&runtimes_dir)
        .map_err(|e| format!("failed to create runtimes directory: {e}"))?;

    let temp_dir = runtimes_dir.join(format!(".python-wasm-install-{}", std::process::id()));
    if temp_dir.exists() {
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("failed to create temp directory: {e}"))?;

    // Extract tarball
    let decoder = GzDecoder::new(std::io::Cursor::new(&bytes));
    let mut archive = Archive::new(decoder);
    if let Err(e) = archive.unpack(&temp_dir) {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("failed to extract tarball: {e}"));
    }

    // Arrange into normalized layout: bin/python.wasm, lib/python3.12/, lib/python312.zip
    let final_dir = runtimes_dir.join(format!(".python-wasm-staged-{}", std::process::id()));
    if final_dir.exists() {
        let _ = std::fs::remove_dir_all(&final_dir);
    }

    let bin_dir = final_dir.join("bin");
    let lib_dir = final_dir.join("lib");
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("failed to create bin dir: {e}"))?;
    std::fs::create_dir_all(&lib_dir)
        .map_err(|e| format!("failed to create lib dir: {e}"))?;

    // Find python*.wasm in extracted archive
    let extracted_bin = temp_dir.join("bin");
    let wasm_file = find_python_wasm_sync(&extracted_bin).ok_or_else(|| {
        "no python*.wasm found in extracted archive".to_string()
    })?;

    std::fs::copy(&wasm_file, bin_dir.join("python.wasm"))
        .map_err(|e| format!("failed to copy python.wasm: {e}"))?;

    // Copy stdlib
    let stdlib_src = temp_dir
        .join("usr")
        .join("local")
        .join("lib")
        .join("python3.12");
    if stdlib_src.exists() {
        copy_dir_all_sync(&stdlib_src, &lib_dir.join("python3.12"))
            .map_err(|e| format!("failed to copy stdlib: {e}"))?;
    } else {
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err("python3.12 stdlib not found in extracted archive".to_string());
    }

    // Copy python312.zip
    let zip_src = temp_dir
        .join("usr")
        .join("local")
        .join("lib")
        .join("python312.zip");
    if zip_src.exists() {
        std::fs::copy(&zip_src, lib_dir.join("python312.zip"))
            .map_err(|e| format!("failed to copy python312.zip: {e}"))?;
    } else {
        let _ = std::fs::remove_dir_all(&temp_dir);
        let _ = std::fs::remove_dir_all(&final_dir);
        return Err("python312.zip not found in extracted archive".to_string());
    }

    // Clean up extraction temp
    let _ = std::fs::remove_dir_all(&temp_dir);

    // Atomic rename into final location
    if target_dir.exists() {
        let _ = std::fs::remove_dir_all(&target_dir);
    }
    std::fs::rename(&final_dir, &target_dir)
        .map_err(|e| format!("failed to install python-wasm runtime: {e}"))?;

    tracing::info!(
        path = %target_dir.display(),
        "CPython WASI runtime installed successfully"
    );

    // Resolve again to validate the installation
    resolve_python_wasm(Some(hivemind_home)).ok_or_else(|| {
        "CPython WASI runtime installed but validation failed".to_string()
    })
}

/// Find the first python*.wasm file in a directory.
fn find_python_wasm_sync(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with("python") && name_str.ends_with(".wasm") {
            return Some(entry.path());
        }
    }
    None
}

/// Recursively copy a directory tree (blocking).
fn copy_dir_all_sync(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let dest_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all_sync(&entry.path(), &dest_path)?;
        } else {
            std::fs::copy(entry.path(), &dest_path)?;
        }
    }
    Ok(())
}
