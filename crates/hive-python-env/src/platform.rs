//! Platform-specific helpers for uv binary and Python venv paths.

/// Returns the uv archive name for the current platform.
pub fn uv_archive_name(_version: &str) -> String {
    let target = uv_target_triple();
    let ext = if cfg!(target_os = "windows") { "zip" } else { "tar.gz" };
    format!("uv-{target}.{ext}")
}

/// Returns the uv download URL for the given version.
pub fn uv_download_url(version: &str) -> String {
    let archive = uv_archive_name(version);
    format!("https://github.com/astral-sh/uv/releases/download/{version}/{archive}")
}

/// Returns the uv binary name for the current platform.
pub fn uv_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "uv.exe"
    } else {
        "uv"
    }
}

/// Returns the python binary path relative to a venv root.
pub fn venv_python_relative() -> &'static str {
    if cfg!(target_os = "windows") {
        "Scripts\\python.exe"
    } else {
        "bin/python3"
    }
}

/// Returns the venv bin directory name.
pub fn venv_bin_dir() -> &'static str {
    if cfg!(target_os = "windows") {
        "Scripts"
    } else {
        "bin"
    }
}

/// Returns the PATH separator for the current platform.
pub fn path_separator() -> &'static str {
    if cfg!(target_os = "windows") {
        ";"
    } else {
        ":"
    }
}

fn uv_target_triple() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "aarch64-apple-darwin",
        ("macos", "x86_64") => "x86_64-apple-darwin",
        ("linux", "x86_64") => "x86_64-unknown-linux-musl",
        ("linux", "aarch64") => "aarch64-unknown-linux-musl",
        ("windows", "x86_64") => "x86_64-pc-windows-msvc",
        (os, arch) => {
            tracing::warn!("unsupported platform {os}/{arch} for uv download");
            "x86_64-unknown-linux-musl"
        }
    }
}
