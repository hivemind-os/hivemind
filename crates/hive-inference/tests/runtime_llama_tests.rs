//! Integration tests for the llama.cpp runtime.

#[cfg(feature = "llama-cpp")]
mod llama_tests {
    use hive_inference::runtime::{
        InferenceError, InferenceRequest, InferenceRuntime, LlamaCppRuntime,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn kind_is_llama_cpp() {
        let rt = LlamaCppRuntime::new();
        assert_eq!(rt.kind(), hive_core::InferenceRuntimeKind::LlamaCpp);
    }

    #[test]
    fn is_available() {
        let rt = LlamaCppRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn supported_formats_include_gguf() {
        let rt = LlamaCppRuntime::new();
        let formats = rt.supported_formats();
        assert!(formats.contains(&"gguf".to_string()));
        assert!(formats.contains(&"ggml".to_string()));
    }

    #[test]
    fn info_has_correct_kind() {
        let rt = LlamaCppRuntime::new();
        let info = rt.info();
        assert_eq!(info.kind, hive_core::InferenceRuntimeKind::LlamaCpp);
        assert!(!info.version.is_empty());
    }

    #[test]
    fn info_shows_no_model_initially() {
        let rt = LlamaCppRuntime::new();
        let info = rt.info();
        assert!(info.loaded_model.is_none());
        assert_eq!(info.memory_used_bytes, 0);
    }

    #[test]
    fn model_not_loaded_initially() {
        let rt = LlamaCppRuntime::new();
        assert!(!rt.is_model_loaded("any-model"));
    }

    #[test]
    fn load_missing_file_returns_error() {
        let rt = LlamaCppRuntime::new();
        let result = rt.load_model("test", Path::new("/nonexistent/model.gguf"));
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
        fs::write(&model_path, b"not a gguf file").unwrap();

        let rt = LlamaCppRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::UnsupportedFormat { .. }) => {}
            other => panic!("Expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn load_invalid_gguf_data_fails() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.gguf");
        fs::write(&model_path, b"this is not valid GGUF data at all").unwrap();

        let rt = LlamaCppRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::LoadFailed(msg)) => {
                assert!(msg.contains("Failed") || msg.contains("load"));
            }
            other => panic!("Expected LoadFailed, got {other:?}"),
        }
    }

    #[test]
    fn infer_on_unloaded_model_fails() {
        let rt = LlamaCppRuntime::new();
        let req = InferenceRequest { prompt: "Hello".into(), ..Default::default() };
        let result = rt.infer("not-loaded", &req);
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn embed_on_unloaded_model_fails() {
        let rt = LlamaCppRuntime::new();
        let result = rt.embed("not-loaded", "test text");
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn unload_nonexistent_model_is_ok() {
        let rt = LlamaCppRuntime::new();
        let result = rt.unload_model("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn multiple_model_ids_are_independent() {
        let rt = LlamaCppRuntime::new();
        assert!(!rt.is_model_loaded("model-a"));
        assert!(!rt.is_model_loaded("model-b"));
    }

    #[test]
    fn info_memory_starts_at_zero() {
        let rt = LlamaCppRuntime::new();
        let info = rt.info();
        assert_eq!(info.memory_used_bytes, 0);
    }

    #[test]
    #[ignore = "Requires a real GGUF model file (e.g., gemma-3-4b-it-Q4_K_M.gguf)"]
    fn load_real_gguf_model_and_infer() {
        let model_path = std::env::var("LLAMA_TEST_MODEL_PATH")
            .unwrap_or_else(|_| "models/gemma-3-4b-it-Q4_K_M.gguf".to_string());
        let model_path = Path::new(&model_path);

        let rt = LlamaCppRuntime::new();
        rt.load_model("gemma", model_path).unwrap();
        assert!(rt.is_model_loaded("gemma"));

        let info = rt.info();
        assert!(info.memory_used_bytes > 0);

        let req = InferenceRequest {
            prompt: "Hello! How are you?".into(),
            max_tokens: Some(50),
            temperature: Some(0.7),
            ..Default::default()
        };
        let output = rt.infer("gemma", &req).unwrap();
        assert!(!output.text.is_empty());
        assert!(output.tokens_used > 0);

        rt.unload_model("gemma").unwrap();
        assert!(!rt.is_model_loaded("gemma"));
    }

    #[test]
    #[ignore = "Requires a real GGUF model file with embedding support"]
    fn load_real_gguf_model_and_embed() {
        let model_path = std::env::var("LLAMA_TEST_MODEL_PATH")
            .unwrap_or_else(|_| "models/gemma-3-4b-it-Q4_K_M.gguf".to_string());
        let model_path = Path::new(&model_path);

        let rt = LlamaCppRuntime::new();
        rt.load_model("embed-test", model_path).unwrap();

        let embedding = rt.embed("embed-test", "Hello world").unwrap();
        assert!(!embedding.is_empty());
        assert!(embedding.iter().all(|v| v.is_finite()));

        rt.unload_model("embed-test").unwrap();
    }
}
