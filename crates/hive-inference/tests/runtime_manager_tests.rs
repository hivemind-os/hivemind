//! Integration tests for the RuntimeManager.

use hive_core::InferenceRuntimeKind;
use hive_inference::{InferenceError, InferenceRequest, RuntimeManager};
use std::path::{Path, PathBuf};

/// Locate vendor model directory.
fn vendor_model_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("ONNX_TEST_MODEL_DIR") {
        let p = PathBuf::from(dir);
        if p.join("model.onnx").exists() {
            return Some(p);
        }
    }
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for _ in 0..5 {
        let candidate = dir.join("vendor/bge-small-en-v1.5");
        if candidate.join("model.onnx").exists() {
            return Some(candidate);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}
use tempfile::tempdir;

#[test]
fn manager_creates_with_all_three_runtimes() {
    let mgr = RuntimeManager::new(4);
    assert!(mgr.get_runtime(InferenceRuntimeKind::Candle).is_some());
    assert!(mgr.get_runtime(InferenceRuntimeKind::Onnx).is_some());
    assert!(mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).is_some());
}

#[test]
fn manager_available_runtimes_has_three_entries() {
    let mgr = RuntimeManager::new(4);
    let kinds = mgr.available_runtimes();
    assert_eq!(kinds.len(), 3);
}

#[test]
fn manager_available_runtimes_have_correct_kinds() {
    let mgr = RuntimeManager::new(4);
    let kinds = mgr.available_runtimes();
    assert!(kinds.contains(&InferenceRuntimeKind::Candle));
    assert!(kinds.contains(&InferenceRuntimeKind::Onnx));
    assert!(kinds.contains(&InferenceRuntimeKind::LlamaCpp));
}

#[test]
fn manager_not_loaded_initially() {
    let mgr = RuntimeManager::new(4);
    assert!(!mgr.is_loaded("anything"));
}

#[test]
fn manager_load_missing_file_fails() {
    let mgr = RuntimeManager::new(4);
    let result = mgr.load_model(
        "missing",
        Path::new("/does/not/exist.gguf"),
        InferenceRuntimeKind::LlamaCpp,
    );
    assert!(result.is_err());
}

#[test]
fn manager_infer_unloaded_model_fails() {
    let mgr = RuntimeManager::new(4);
    let req = InferenceRequest { prompt: "Hello".into(), ..Default::default() };
    let result = mgr.infer("nonexistent", &req);
    assert!(result.is_err());
    match result {
        Err(InferenceError::ModelNotLoaded { model_id }) => {
            assert_eq!(model_id, "nonexistent");
        }
        other => panic!("Expected ModelNotLoaded, got {other:?}"),
    }
}

#[test]
fn manager_embed_unloaded_model_fails() {
    let mgr = RuntimeManager::new(4);
    let result = mgr.embed("nonexistent", "hello");
    assert!(result.is_err());
}

#[test]
fn manager_unload_nonexistent_is_ok() {
    let mgr = RuntimeManager::new(4);
    let result = mgr.unload_model("nonexistent");
    assert!(result.is_ok());
}

#[test]
fn manager_unload_cleans_loaded_state() {
    let mgr = RuntimeManager::new(4);
    // Unload something that was never loaded
    mgr.unload_model("test").unwrap();
    assert!(!mgr.is_loaded("test"));
}

#[test]
fn manager_get_runtime_returns_correct_kind() {
    let mgr = RuntimeManager::new(4);
    let candle = mgr.get_runtime(InferenceRuntimeKind::Candle).unwrap();
    assert_eq!(candle.kind(), InferenceRuntimeKind::Candle);
    let onnx = mgr.get_runtime(InferenceRuntimeKind::Onnx).unwrap();
    assert_eq!(onnx.kind(), InferenceRuntimeKind::Onnx);
    let llama = mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).unwrap();
    assert_eq!(llama.kind(), InferenceRuntimeKind::LlamaCpp);
}

#[test]
fn manager_runtimes_are_available() {
    let mgr = RuntimeManager::new(4);
    for kind in mgr.available_runtimes() {
        let rt = mgr.get_runtime(kind).unwrap();
        assert!(rt.is_available());
    }
}

#[test]
fn manager_each_runtime_has_supported_formats() {
    let mgr = RuntimeManager::new(4);
    let candle = mgr.get_runtime(InferenceRuntimeKind::Candle).unwrap();
    assert!(!candle.supported_formats().is_empty());
    let onnx = mgr.get_runtime(InferenceRuntimeKind::Onnx).unwrap();
    assert!(!onnx.supported_formats().is_empty());
    let llama = mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).unwrap();
    assert!(!llama.supported_formats().is_empty());
}

#[test]
fn manager_formats_are_disjoint_across_runtimes() {
    let mgr = RuntimeManager::new(4);
    let candle_fmts = mgr.get_runtime(InferenceRuntimeKind::Candle).unwrap().supported_formats();
    let onnx_fmts = mgr.get_runtime(InferenceRuntimeKind::Onnx).unwrap().supported_formats();
    let llama_fmts = mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).unwrap().supported_formats();

    // ONNX only handles .onnx
    assert!(onnx_fmts.contains(&"onnx".to_string()));
    assert!(!candle_fmts.contains(&"onnx".to_string()));
    // LlamaCpp only handles .gguf/.ggml
    assert!(llama_fmts.contains(&"gguf".to_string()));
    assert!(!onnx_fmts.contains(&"gguf".to_string()));
    // Candle handles .safetensors
    assert!(candle_fmts.contains(&"safetensors".to_string()));
    assert!(!llama_fmts.contains(&"safetensors".to_string()));
}

#[test]
fn manager_load_wrong_format_for_runtime_fails() {
    let dir = tempdir().unwrap();
    let gguf_path = dir.path().join("model.gguf");
    std::fs::write(&gguf_path, b"fake").unwrap();

    let mgr = RuntimeManager::new(4);
    // Try loading a GGUF file with the Candle runtime
    let result = mgr.load_model("test", &gguf_path, InferenceRuntimeKind::Candle);
    // Should fail because Candle doesn't support GGUF
    assert!(result.is_err());
}

#[test]
#[ignore = "Requires real model files for multiple runtimes"]
fn manager_eviction_with_real_models() {
    let model_dir =
        std::env::var("MANAGER_TEST_MODEL_DIR").unwrap_or_else(|_| "models".to_string());
    let gguf_path = Path::new(&model_dir).join("test.gguf");

    let mgr = RuntimeManager::new(2);
    mgr.load_model("m1", &gguf_path, InferenceRuntimeKind::LlamaCpp).unwrap();
    mgr.load_model("m2", &gguf_path, InferenceRuntimeKind::LlamaCpp).unwrap();
    // Third load should evict one model
    mgr.load_model("m3", &gguf_path, InferenceRuntimeKind::LlamaCpp).unwrap();

    assert!(mgr.is_loaded("m3"));
}

#[test]
fn manager_embed_with_onnx_runtime() {
    let model_dir = match vendor_model_dir() {
        Some(d) => d,
        None => {
            eprintln!(
                "WARNING: skipping manager embed test — model not found. \
                 Run `cargo xtask fetch-models` to download it."
            );
            return;
        }
    };

    let mgr = RuntimeManager::new(4);
    let onnx_path = model_dir.join("model.onnx");
    mgr.load_model("embed", &onnx_path, InferenceRuntimeKind::Onnx).unwrap();

    let embedding = mgr.embed("embed", "test sentence").unwrap();
    assert_eq!(embedding.len(), 384);
    assert!(embedding.iter().all(|v| v.is_finite()));
}

#[test]
#[ignore = "Requires GGUF model file"]
fn manager_cross_runtime_inference() {
    let model_dir =
        std::env::var("MANAGER_TEST_MODEL_DIR").unwrap_or_else(|_| "models".to_string());

    let mgr = RuntimeManager::new(4);

    // Load a GGUF model for llama.cpp
    let gguf_path = PathBuf::from(&model_dir).join("test.gguf");
    mgr.load_model("chat", &gguf_path, InferenceRuntimeKind::LlamaCpp).unwrap();

    // Load an ONNX model for embeddings
    let onnx_path = PathBuf::from(&model_dir).join("bge-small-en-v1.5/model.onnx");
    mgr.load_model("embed", &onnx_path, InferenceRuntimeKind::Onnx).unwrap();

    // Infer with llama.cpp (dispatched automatically)
    let req =
        InferenceRequest { prompt: "Hello".into(), max_tokens: Some(10), ..Default::default() };
    let output = mgr.infer("chat", &req).unwrap();
    assert!(!output.text.is_empty());

    // Embed with ONNX (dispatched automatically)
    let embedding = mgr.embed("embed", "test sentence").unwrap();
    assert!(!embedding.is_empty());
}
