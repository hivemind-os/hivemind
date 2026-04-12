//! Integration tests for the ONNX runtime.

#[cfg(feature = "onnx")]
mod onnx_tests {
    use hive_inference::runtime::{
        InferenceError, InferenceRequest, InferenceRuntime, OnnxRuntime,
    };
    use std::fs;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    /// Locate the vendor model directory. Checks env var first,
    /// then walks up from CARGO_MANIFEST_DIR to find `vendor/bge-small-en-v1.5`.
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

    #[test]
    fn kind_is_onnx() {
        let rt = OnnxRuntime::new();
        assert_eq!(rt.kind(), hive_core::InferenceRuntimeKind::Onnx);
    }

    #[test]
    fn is_available() {
        let rt = OnnxRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn supported_formats_is_onnx_only() {
        let rt = OnnxRuntime::new();
        let formats = rt.supported_formats();
        assert_eq!(formats, vec!["onnx".to_string()]);
    }

    #[test]
    fn info_has_correct_kind() {
        let rt = OnnxRuntime::new();
        let info = rt.info();
        assert_eq!(info.kind, hive_core::InferenceRuntimeKind::Onnx);
        assert!(!info.version.is_empty());
    }

    #[test]
    fn info_shows_no_model_initially() {
        let rt = OnnxRuntime::new();
        let info = rt.info();
        assert!(info.loaded_model.is_none());
        assert_eq!(info.memory_used_bytes, 0);
    }

    #[test]
    fn model_not_loaded_initially() {
        let rt = OnnxRuntime::new();
        assert!(!rt.is_model_loaded("any-model"));
    }

    #[test]
    fn load_missing_file_returns_error() {
        let rt = OnnxRuntime::new();
        let result = rt.load_model("test", Path::new("/nonexistent/model.onnx"));
        assert!(result.is_err());
        match result {
            Err(InferenceError::ModelFileNotFound(_)) => {}
            other => panic!("Expected ModelFileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_wrong_format_returns_error() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.safetensors");
        fs::write(&model_path, b"not an onnx file").unwrap();

        let rt = OnnxRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::UnsupportedFormat { .. }) => {}
            other => panic!("Expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn load_invalid_onnx_data_fails() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.onnx");
        fs::write(&model_path, b"this is not valid ONNX protobuf data").unwrap();

        let rt = OnnxRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::LoadFailed(msg)) => {
                assert!(msg.contains("Failed") || msg.contains("load") || msg.contains("ONNX"));
            }
            other => panic!("Expected LoadFailed, got {other:?}"),
        }
    }

    #[test]
    fn infer_on_unloaded_model_fails() {
        let rt = OnnxRuntime::new();
        let req = InferenceRequest { prompt: "Hello".into(), ..Default::default() };
        let result = rt.infer("not-loaded", &req);
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn embed_on_unloaded_model_fails() {
        let rt = OnnxRuntime::new();
        let result = rt.embed("not-loaded", "test text");
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn unload_nonexistent_model_is_ok() {
        let rt = OnnxRuntime::new();
        let result = rt.unload_model("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn unload_removes_model() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.onnx");
        fs::write(&model_path, b"fake onnx").unwrap();

        let rt = OnnxRuntime::new();
        // Even though load will fail on invalid data, unload should work
        let _ = rt.load_model("test", &model_path);
        rt.unload_model("test").unwrap();
        assert!(!rt.is_model_loaded("test"));
    }

    #[test]
    fn multiple_model_ids_independent() {
        let rt = OnnxRuntime::new();
        assert!(!rt.is_model_loaded("model-a"));
        assert!(!rt.is_model_loaded("model-b"));
        assert!(!rt.is_model_loaded("model-c"));
    }

    #[test]
    fn info_supports_gpu() {
        let rt = OnnxRuntime::new();
        let info = rt.info();
        // ONNX Runtime can support GPU via CUDA/DirectML
        assert!(info.supports_gpu);
    }

    #[test]
    #[test]
    fn load_real_bge_onnx_model_and_embed() {
        let model_dir = match vendor_model_dir() {
            Some(d) => d,
            None => {
                eprintln!(
                    "WARNING: skipping embedding test — model not found. \
                     Run `cargo xtask fetch-models` to download it."
                );
                return;
            }
        };
        let model_path = model_dir.join("model.onnx");

        let rt = OnnxRuntime::new();
        rt.load_model("bge", &model_path).unwrap();
        assert!(rt.is_model_loaded("bge"));

        let info = rt.info();
        assert!(info.memory_used_bytes > 0);

        let embedding = rt.embed("bge", "Hello world").unwrap();
        assert_eq!(embedding.len(), 384);
        assert!(embedding.iter().all(|v| v.is_finite()));

        // Embeddings should be deterministic
        let embedding2 = rt.embed("bge", "Hello world").unwrap();
        assert_eq!(embedding, embedding2);

        // Different text should give different embeddings
        let embedding3 = rt.embed("bge", "Something completely different").unwrap();
        assert_ne!(embedding, embedding3);

        rt.unload_model("bge").unwrap();
        assert!(!rt.is_model_loaded("bge"));
    }
}
