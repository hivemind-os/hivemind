//! Download and extract the uv binary.

use crate::{platform, PythonEnvError};
use std::path::{Path, PathBuf};

/// Download the uv binary to the given directory, returning the path to the binary.
///
/// Uses atomic download (write to `.partial`, then rename) to avoid partial files.
pub async fn ensure_uv_binary(uv_dir: &Path, version: &str) -> Result<PathBuf, PythonEnvError> {
    let binary_path = uv_dir.join(platform::uv_binary_name());
    if binary_path.exists() {
        tracing::debug!("uv binary already exists at {}", binary_path.display());
        return Ok(binary_path);
    }

    std::fs::create_dir_all(uv_dir)?;
    let url = platform::uv_download_url(version);
    tracing::info!("downloading uv {version} from {url}");

    let response = reqwest::get(&url)
        .await
        .map_err(|e| PythonEnvError::Download(format!("failed to download uv from {url}: {e}")))?;

    if !response.status().is_success() {
        return Err(PythonEnvError::Download(format!(
            "uv download returned HTTP {}",
            response.status()
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| PythonEnvError::Download(format!("failed to read uv download body: {e}")))?;

    let partial_path = uv_dir.join(format!("{}.partial", platform::uv_binary_name()));

    if cfg!(target_os = "windows") {
        extract_zip(&bytes, uv_dir, &partial_path)?;
    } else {
        extract_tar_gz(&bytes, uv_dir, &partial_path)?;
    }

    if partial_path.exists() {
        std::fs::rename(&partial_path, &binary_path)?;
    }

    // The archive extracts `uv` directly into the dir on some platforms.
    if !binary_path.exists() {
        return Err(PythonEnvError::Extraction(format!(
            "uv binary not found at {} after extraction",
            binary_path.display()
        )));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary_path, std::fs::Permissions::from_mode(0o755))?;
    }

    tracing::info!("uv binary installed at {}", binary_path.display());
    Ok(binary_path)
}

fn extract_tar_gz(
    data: &[u8],
    dest_dir: &Path,
    _partial_path: &Path,
) -> Result<(), PythonEnvError> {
    let gz = flate2::read::GzDecoder::new(data);
    let mut archive = tar::Archive::new(gz);

    for entry in archive.entries().map_err(|e| PythonEnvError::Extraction(e.to_string()))? {
        let mut entry = entry.map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
        let path = entry.path().map_err(|e| PythonEnvError::Extraction(e.to_string()))?;

        // uv archives contain files like `uv-<triple>/uv` — extract the binary.
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name == platform::uv_binary_name() {
            let dest = dest_dir.join(platform::uv_binary_name());
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
            std::io::copy(&mut entry, &mut out)
                .map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
            return Ok(());
        }
    }

    Err(PythonEnvError::Extraction("uv binary not found in tar.gz archive".to_string()))
}

fn extract_zip(data: &[u8], dest_dir: &Path, _partial_path: &Path) -> Result<(), PythonEnvError> {
    let reader = std::io::Cursor::new(data);
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| PythonEnvError::Extraction(e.to_string()))?;

    for i in 0..archive.len() {
        let mut file =
            archive.by_index(i).map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
        let name = file.name().to_string();
        let file_name =
            std::path::Path::new(&name).file_name().and_then(|n| n.to_str()).unwrap_or("");

        if file_name == platform::uv_binary_name() {
            let dest = dest_dir.join(platform::uv_binary_name());
            let mut out = std::fs::File::create(&dest)
                .map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
            std::io::copy(&mut file, &mut out)
                .map_err(|e| PythonEnvError::Extraction(e.to_string()))?;
            return Ok(());
        }
    }

    Err(PythonEnvError::Extraction("uv binary not found in zip archive".to_string()))
}
