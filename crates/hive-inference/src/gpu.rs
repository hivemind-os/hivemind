//! GPU acceleration helpers: VRAM-based layer recommendation.

/// Headroom to leave free in VRAM for OS compositor, other apps, and KV cache overhead.
const VRAM_HEADROOM_BYTES: u64 = 512 * 1024 * 1024; // 512 MB

/// Recommend the number of model layers to offload to GPU based on available VRAM.
///
/// # Arguments
/// * `model_size_bytes` — size of the GGUF file on disk (approximates weight memory)
/// * `n_layers` — total number of layers in the model (from GGUF metadata or estimate)
/// * `vram_bytes` — total VRAM reported by the GPU
///
/// # Returns
/// Recommended number of layers to offload (0 if none fit, capped at `n_layers`).
pub fn recommend_gpu_layers(model_size_bytes: u64, n_layers: u32, vram_bytes: u64) -> u32 {
    if n_layers == 0 || vram_bytes == 0 || model_size_bytes == 0 {
        return 0;
    }

    let usable_vram = vram_bytes.saturating_sub(VRAM_HEADROOM_BYTES);
    if usable_vram == 0 {
        return 0;
    }

    // Approximate per-layer memory cost: model file size / number of layers.
    // GGUF files store all layers plus embeddings/output heads, so this slightly
    // overestimates per-layer cost which makes the recommendation conservative.
    let per_layer_bytes = model_size_bytes / n_layers as u64;
    if per_layer_bytes == 0 {
        return n_layers; // degenerate: model is tiny, offload everything
    }

    let layers_that_fit = (usable_vram / per_layer_bytes) as u32;
    layers_that_fit.min(n_layers)
}

/// Estimate the number of transformer layers in a GGUF model based on file size.
/// This is a rough heuristic when metadata is unavailable.
///
/// Common patterns:
/// - 7B params ≈ 4-5 GB (Q4) → ~32 layers
/// - 13B params ≈ 7-8 GB (Q4) → ~40 layers
/// - 70B params ≈ 35-40 GB (Q4) → ~80 layers
pub fn estimate_layer_count(model_size_bytes: u64) -> u32 {
    let gb = model_size_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    match gb {
        x if x < 1.0 => 12,  // very small models (1-3B quantized)
        x if x < 3.0 => 22,  // ~3B models
        x if x < 6.0 => 32,  // ~7B models
        x if x < 12.0 => 40, // ~13B models
        x if x < 25.0 => 60, // ~33-34B models
        _ => 80,              // 70B+ models
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_vram_returns_zero() {
        assert_eq!(recommend_gpu_layers(4_000_000_000, 32, 0), 0);
    }

    #[test]
    fn test_tiny_vram_returns_zero() {
        // 256MB VRAM is below headroom
        assert_eq!(recommend_gpu_layers(4_000_000_000, 32, 256 * 1024 * 1024), 0);
    }

    #[test]
    fn test_enough_vram_for_all_layers() {
        // 16GB VRAM, 4GB model with 32 layers → all fit
        let vram = 16 * 1024 * 1024 * 1024u64;
        let model = 4 * 1024 * 1024 * 1024u64;
        assert_eq!(recommend_gpu_layers(model, 32, vram), 32);
    }

    #[test]
    fn test_partial_offload() {
        // 4GB VRAM, 8GB model with 40 layers
        // Usable = 4GB - 512MB = 3.5GB
        // Per layer = 8GB / 40 = ~200MB
        // Layers that fit = 3.5GB / 200MB = ~17
        let vram = 4 * 1024 * 1024 * 1024u64;
        let model = 8 * 1024 * 1024 * 1024u64;
        let layers = recommend_gpu_layers(model, 40, vram);
        assert!(layers > 0 && layers < 40, "got {layers}");
    }

    #[test]
    fn test_estimate_layer_count() {
        assert_eq!(estimate_layer_count(500_000_000), 12);
        assert_eq!(estimate_layer_count(4_500_000_000), 32);
        assert_eq!(estimate_layer_count(8_000_000_000), 40);
        assert_eq!(estimate_layer_count(40_000_000_000), 80);
    }
}
