//! Platform-specific helpers for Node.js binary distribution paths.

/// Returns the Node.js distribution archive name for the current platform.
///
/// Examples:
/// - `node-v22.16.0-darwin-arm64.tar.gz`
/// - `node-v22.16.0-linux-x64.tar.xz`
/// - `node-v22.16.0-win-x64.zip`
pub fn node_archive_name(version: &str) -> String {
    let (platform, arch) = node_platform_arch();
    let ext = archive_extension();
    format!("node-v{version}-{platform}-{arch}.{ext}")
}

/// Returns the Node.js download URL for the given version.
pub fn node_download_url(version: &str) -> String {
    let archive = node_archive_name(version);
    format!("https://nodejs.org/dist/v{version}/{archive}")
}

/// Returns the directory name inside the archive (the top-level folder).
pub fn node_archive_dir_name(version: &str) -> String {
    let (platform, arch) = node_platform_arch();
    format!("node-v{version}-{platform}-{arch}")
}

/// Returns the name of the node binary for the current platform.
pub fn node_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "node.exe"
    } else {
        "node"
    }
}

/// Returns the name of the npm entry point for the current platform.
#[allow(dead_code)]
pub fn npm_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "npm.cmd"
    } else {
        "npm"
    }
}

/// Returns the name of the npx entry point for the current platform.
#[allow(dead_code)]
pub fn npx_binary_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "npx.cmd"
    } else {
        "npx"
    }
}

/// Returns the `bin` directory relative to the extracted Node.js distribution root.
pub fn node_bin_dir() -> &'static str {
    if cfg!(target_os = "windows") {
        // Windows Node.js zip puts binaries in the root, not a `bin/` subdirectory.
        ""
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

/// Returns the archive file extension for the current platform.
pub fn archive_extension() -> &'static str {
    match std::env::consts::OS {
        "windows" => "zip",
        "linux" => "tar.xz",
        _ => "tar.gz",
    }
}

/// Returns (platform, arch) strings matching Node.js distribution naming.
fn node_platform_arch() -> (&'static str, &'static str) {
    let platform = match std::env::consts::OS {
        "macos" => "darwin",
        "linux" => "linux",
        "windows" => "win",
        os => {
            tracing::warn!("unsupported OS {os} for Node.js download, defaulting to linux");
            "linux"
        }
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        a => {
            tracing::warn!("unsupported arch {a} for Node.js download, defaulting to x64");
            "x64"
        }
    };
    (platform, arch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_name_contains_version() {
        let name = node_archive_name("22.16.0");
        assert!(name.contains("22.16.0"));
        assert!(name.starts_with("node-v"));
    }

    #[test]
    fn download_url_is_valid() {
        let url = node_download_url("22.16.0");
        assert!(url.starts_with("https://nodejs.org/dist/v22.16.0/"));
        assert!(url.contains("node-v22.16.0"));
    }

    #[test]
    fn archive_dir_name_matches() {
        let dir = node_archive_dir_name("22.16.0");
        assert!(dir.starts_with("node-v22.16.0-"));
    }

    #[test]
    fn binary_names_are_non_empty() {
        assert!(!node_binary_name().is_empty());
        assert!(!npm_binary_name().is_empty());
        assert!(!npx_binary_name().is_empty());
    }
}
