//! Default model configurations for local inference.

/// Default chat model: Google Gemma 3 4B Instruct (GGUF quantized for llama.cpp)
pub const DEFAULT_CHAT_MODEL_REPO: &str = "google/gemma-3-4b-it";
pub const DEFAULT_CHAT_MODEL_FILE: &str = "gemma-3-4b-it-Q4_K_M.gguf";
pub const DEFAULT_CHAT_MODEL_ID: &str = "gemma-3-4b-it";

/// Default embedding model: BGE-small-en-v1.5 (ONNX format)
pub const DEFAULT_EMBEDDING_MODEL_REPO: &str = "BAAI/bge-small-en-v1.5";
pub const DEFAULT_EMBEDDING_MODEL_FILE: &str = "model.onnx";
pub const DEFAULT_EMBEDDING_TOKENIZER_FILE: &str = "tokenizer.json";
pub const DEFAULT_EMBEDDING_MODEL_ID: &str = "bge-small-en-v1.5";
pub const DEFAULT_EMBEDDING_DIMENSION: usize = 384;

/// Get the HuggingFace repo ID for the default chat model.
pub fn default_chat_repo() -> &'static str {
    DEFAULT_CHAT_MODEL_REPO
}

/// Get the GGUF filename for the default chat model.
pub fn default_chat_filename() -> &'static str {
    DEFAULT_CHAT_MODEL_FILE
}

/// Get the model ID string for the default chat model.
pub fn default_chat_model_id() -> &'static str {
    DEFAULT_CHAT_MODEL_ID
}

/// Get the HuggingFace repo ID for the default embedding model.
pub fn default_embedding_repo() -> &'static str {
    DEFAULT_EMBEDDING_MODEL_REPO
}

/// Get the ONNX filename for the default embedding model.
pub fn default_embedding_filename() -> &'static str {
    DEFAULT_EMBEDDING_MODEL_FILE
}

/// Get the model ID string for the default embedding model.
pub fn default_embedding_model_id() -> &'static str {
    DEFAULT_EMBEDDING_MODEL_ID
}

/// Get the embedding dimension for the default embedding model (BGE-small: 384).
pub fn default_embedding_dimension() -> usize {
    DEFAULT_EMBEDDING_DIMENSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_chat_model_config() {
        assert!(default_chat_repo().contains("gemma"));
        assert!(default_chat_filename().ends_with(".gguf"));
        assert!(!default_chat_model_id().is_empty());
    }

    #[test]
    fn default_embedding_model_config() {
        assert!(default_embedding_repo().contains("bge"));
        assert!(default_embedding_filename().ends_with(".onnx"));
        assert_eq!(default_embedding_dimension(), 384);
        assert!(!default_embedding_model_id().is_empty());
    }
}
