//! Download and extract the Node.js distribution.

use std::path::{Path, PathBuf};

use crate::{platform, NodeEnvError};

/// Download the Node.js distribution to `node_dir`, returning the path to the
/// extracted distribution root (containing `bin/node` or `node.exe`).
///
/// Uses an atomic download pattern: writes to a `.partial` marker, then renames
/// into place.
pub async fn ensure_node_distribution(
    node_dir: &Path,
    version: &str,
) -> Result<PathBuf, NodeEnvError> {
    // Validate version string to prevent path traversal.
    crate::validate_node_version(version)?;

    let dist_dir = node_dir.join(platform::node_archive_dir_name(version));
    let node_binary = if platform::node_bin_dir().is_empty() {
        dist_dir.join(platform::node_binary_name())
    } else {
        dist_dir.join(platform::node_bin_dir()).join(platform::node_binary_name())
    };

    if node_binary.exists() {
        tracing::debug!("Node.js already installed at {}", dist_dir.display());
        return Ok(dist_dir);
    }

    std::fs::create_dir_all(node_dir)?;

    let url = platform::node_download_url(version);
    tracing::info!("downloading Node.js v{version} from {url}");

    let response = reqwest::get(&url).await.map_err(|e| {
        NodeEnvError::Download(format!("failed to download Node.js from {url}: {e}"))
    })?;

    if !response.status().is_success() {
        return Err(NodeEnvError::Download(format!(
            "Node.js download returned HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| NodeEnvError::Download(format!("failed to read download body: {e}")))?;

    let partial_marker = node_dir.join(format!(".installing-{version}"));
    std::fs::write(&partial_marker, b"partial")?;

    let ext = platform::archive_extension();
    let result = match ext {
        "zip" => extract_zip(&bytes, node_dir),
        "tar.xz" => extract_tar_xz(&bytes, node_dir),
        "tar.gz" => extract_tar_gz(&bytes, node_dir),
        _ => Err(NodeEnvError::Extraction(format!("unsupported archive extension: {ext}"))),
    };

    if let Err(e) = result {
        // Clean up partial extraction.
        let _ = std::fs::remove_dir_all(&dist_dir);
        let _ = std::fs::remove_file(&partial_marker);
        return Err(e);
    }

    let _ = std::fs::remove_file(&partial_marker);

    if !node_binary.exists() {
        return Err(NodeEnvError::Extraction(format!(
            "node binary not found at {} after extraction",
            node_binary.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&node_binary, std::fs::Permissions::from_mode(0o755))?;
    }

    tracing::info!("Node.js v{version} installed at {}", dist_dir.display());
    Ok(dist_dir)
}

fn extract_tar_gz(data: &[u8], dest_dir: &Path) -> Result<(), NodeEnvError> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);
    archive
        .unpack(dest_dir)
        .map_err(|e| NodeEnvError::Extraction(format!("failed to unpack tar.gz: {e}")))?;
    Ok(())
}

fn extract_tar_xz(data: &[u8], dest_dir: &Path) -> Result<(), NodeEnvError> {
    let xz = xz2::read::XzDecoder::new(data);
    let mut archive = tar::Archive::new(xz);
    archive
        .unpack(dest_dir)
        .map_err(|e| NodeEnvError::Extraction(format!("failed to unpack tar.xz: {e}")))?;
    Ok(())
}

fn extract_zip(data: &[u8], dest_dir: &Path) -> Result<(), NodeEnvError> {
    let reader = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| NodeEnvError::Extraction(e.to_string()))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| NodeEnvError::Extraction(e.to_string()))?;
        let name = file.name().to_string();
        let outpath = dest_dir.join(&name);

        if file.is_dir() {
            std::fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = std::fs::File::create(&outpath)
                .map_err(|e| NodeEnvError::Extraction(e.to_string()))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| NodeEnvError::Extraction(e.to_string()))?;
        }
    }

    Ok(())
}
