//! Integration tests for the Candle runtime.

#[cfg(feature = "candle")]
mod candle_tests {
    use hive_inference::runtime::{
        CandleRuntime, InferenceError, InferenceRequest, InferenceRuntime,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn kind_is_candle() {
        let rt = CandleRuntime::new();
        assert_eq!(rt.kind(), hive_core::InferenceRuntimeKind::Candle);
    }

    #[test]
    fn is_always_available() {
        let rt = CandleRuntime::new();
        assert!(rt.is_available());
    }

    #[test]
    fn supported_formats_include_safetensors() {
        let rt = CandleRuntime::new();
        let formats = rt.supported_formats();
        assert!(formats.contains(&"safetensors".to_string()));
        assert!(formats.contains(&"bin".to_string()));
        assert!(formats.contains(&"pt".to_string()));
    }

    #[test]
    fn info_has_correct_kind() {
        let rt = CandleRuntime::new();
        let info = rt.info();
        assert_eq!(info.kind, hive_core::InferenceRuntimeKind::Candle);
        assert!(!info.version.is_empty());
    }

    #[test]
    fn info_shows_no_model_initially() {
        let rt = CandleRuntime::new();
        let info = rt.info();
        assert!(info.loaded_model.is_none());
        assert_eq!(info.memory_used_bytes, 0);
    }

    #[test]
    fn model_not_loaded_initially() {
        let rt = CandleRuntime::new();
        assert!(!rt.is_model_loaded("any-model"));
    }

    #[test]
    fn load_missing_file_returns_error() {
        let rt = CandleRuntime::new();
        let result = rt.load_model("test", Path::new("/nonexistent/model.safetensors"));
        assert!(result.is_err());
        match result {
            Err(InferenceError::ModelFileNotFound(_)) => {}
            other => panic!("Expected ModelFileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn load_wrong_format_returns_error() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.gguf");
        fs::write(&model_path, b"not a safetensors file").unwrap();

        let rt = CandleRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::UnsupportedFormat { .. }) => {}
            other => panic!("Expected UnsupportedFormat, got {other:?}"),
        }
    }

    #[test]
    fn load_safetensors_without_config_fails() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.safetensors");
        fs::write(&model_path, b"fake safetensors data").unwrap();
        // No config.json or tokenizer.json

        let rt = CandleRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::LoadFailed(msg)) => {
                assert!(msg.contains("tokenizer") || msg.contains("config"));
            }
            other => panic!("Expected LoadFailed, got {other:?}"),
        }
    }

    #[test]
    fn infer_on_unloaded_model_fails() {
        let rt = CandleRuntime::new();
        let req = InferenceRequest { prompt: "Hello".into(), ..Default::default() };
        let result = rt.infer("not-loaded", &req);
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn embed_on_unloaded_model_fails() {
        let rt = CandleRuntime::new();
        let result = rt.embed("not-loaded", "test text");
        assert!(matches!(result, Err(InferenceError::ModelNotLoaded { .. })));
    }

    #[test]
    fn unload_nonexistent_model_is_ok() {
        let rt = CandleRuntime::new();
        let result = rt.unload_model("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn load_with_invalid_safetensors_data_fails() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.safetensors");
        fs::write(&model_path, b"this is not valid safetensors data").unwrap();
        // Create a minimal tokenizer.json and config.json
        let tokenizer_json = r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,"vocab":{},"merges":[]}}"#;
        fs::write(dir.path().join("tokenizer.json"), tokenizer_json).unwrap();
        let config_json = r#"{"model_type":"bert","hidden_size":384,"num_hidden_layers":6,"num_attention_heads":6,"intermediate_size":1536,"vocab_size":30522}"#;
        fs::write(dir.path().join("config.json"), config_json).unwrap();

        let rt = CandleRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
    }

    #[test]
    fn load_with_unsupported_architecture_fails() {
        let dir = tempdir().unwrap();
        let model_path = dir.path().join("model.safetensors");
        fs::write(&model_path, b"fake data").unwrap();
        let tokenizer_json = r#"{"version":"1.0","truncation":null,"padding":null,"added_tokens":[],"normalizer":null,"pre_tokenizer":null,"post_processor":null,"decoder":null,"model":{"type":"BPE","dropout":null,"unk_token":null,"continuing_subword_prefix":null,"end_of_word_suffix":null,"fuse_unk":false,"byte_fallback":false,"vocab":{},"merges":[]}}"#;
        fs::write(dir.path().join("tokenizer.json"), tokenizer_json).unwrap();
        let config_json = r#"{"model_type":"gpt2","hidden_size":768,"num_hidden_layers":12}"#;
        fs::write(dir.path().join("config.json"), config_json).unwrap();

        let rt = CandleRuntime::new();
        let result = rt.load_model("test", &model_path);
        assert!(result.is_err());
        match result {
            Err(InferenceError::LoadFailed(msg)) => {
                assert!(msg.contains("gpt2") || msg.contains("Unsupported"));
            }
            other => panic!("Expected LoadFailed for unsupported arch, got {other:?}"),
        }
    }

    #[test]
    fn multiple_models_can_be_tracked() {
        let rt = CandleRuntime::new();
        // Without loading, check multiple model IDs
        assert!(!rt.is_model_loaded("model-a"));
        assert!(!rt.is_model_loaded("model-b"));
        assert!(!rt.is_model_loaded("model-c"));
    }

    #[test]
    #[ignore = "Requires safetensors model files (not downloaded by cargo xtask fetch-models)"]
    fn load_real_bge_model_and_embed() {
        // To run: download BAAI/bge-small-en-v1.5 safetensors + config + tokenizer
        // and set MODEL_DIR env var
        let model_dir = std::env::var("CANDLE_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "models/bge-small-en-v1.5".to_string());
        let model_path = Path::new(&model_dir).join("model.safetensors");

        let rt = CandleRuntime::new();
        rt.load_model("bge", &model_path).unwrap();
        assert!(rt.is_model_loaded("bge"));

        let embedding = rt.embed("bge", "Hello world").unwrap();
        assert_eq!(embedding.len(), 384);
        assert!(embedding.iter().all(|v| v.is_finite()));

        rt.unload_model("bge").unwrap();
        assert!(!rt.is_model_loaded("bge"));
    }
}
