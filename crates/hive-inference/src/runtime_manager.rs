use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;

use crate::runtime::{
    CandleRuntime, InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime,
    LlamaCppRuntime, OnnxRuntime,
};
use crate::worker_proxy::{RuntimeWorkerProxy, WorkerConfig};
use hive_core::InferenceRuntimeKind;

/// Coordinates multiple local inference runtimes and manages model lifecycle
/// with an LRU-style eviction policy.
pub struct RuntimeManager {
    runtimes: HashMap<InferenceRuntimeKind, Arc<dyn InferenceRuntime>>,
    loaded_models: Mutex<HashMap<String, InferenceRuntimeKind>>,
    max_loaded: usize,
}

impl RuntimeManager {
    /// Creates a new `RuntimeManager` with feature-gated in-process runtimes.
    pub fn new(max_loaded_models: usize) -> Self {
        let mut runtimes: HashMap<InferenceRuntimeKind, Arc<dyn InferenceRuntime>> = HashMap::new();

        runtimes.insert(InferenceRuntimeKind::Candle, Arc::new(CandleRuntime::new()));
        runtimes.insert(InferenceRuntimeKind::Onnx, Arc::new(OnnxRuntime::new()));
        runtimes.insert(InferenceRuntimeKind::LlamaCpp, Arc::new(LlamaCppRuntime::new()));

        Self { runtimes, loaded_models: Mutex::new(HashMap::new()), max_loaded: max_loaded_models }
    }

    /// Creates a new `RuntimeManager` that delegates to isolated worker
    /// processes instead of running runtimes in-process. Each runtime kind
    /// gets its own child process communicating over stdio.
    pub fn new_isolated(
        max_loaded_models: usize,
        worker_binary: PathBuf,
        request_timeout: Option<Duration>,
    ) -> Self {
        let mut runtimes: HashMap<InferenceRuntimeKind, Arc<dyn InferenceRuntime>> = HashMap::new();

        for kind in [
            InferenceRuntimeKind::Candle,
            InferenceRuntimeKind::Onnx,
            InferenceRuntimeKind::LlamaCpp,
        ] {
            let mut config = WorkerConfig {
                worker_binary: worker_binary.clone(),
                runtime_kind: kind,
                ..Default::default()
            };
            if let Some(t) = request_timeout {
                config.request_timeout = t;
            }
            runtimes.insert(kind, Arc::new(RuntimeWorkerProxy::new(config)));
        }

        Self { runtimes, loaded_models: Mutex::new(HashMap::new()), max_loaded: max_loaded_models }
    }

    /// Returns the runtime for a given kind, if registered.
    pub fn get_runtime(&self, kind: InferenceRuntimeKind) -> Option<Arc<dyn InferenceRuntime>> {
        self.runtimes.get(&kind).cloned()
    }

    /// Loads a model via the appropriate runtime. Evicts the oldest model if
    /// the number of loaded models would exceed `max_loaded`.
    pub fn load_model(
        &self,
        model_id: &str,
        path: &Path,
        kind: InferenceRuntimeKind,
    ) -> Result<(), InferenceError> {
        let runtime =
            self.runtimes.get(&kind).ok_or_else(|| InferenceError::RuntimeUnavailable {
                runtime: kind,
                reason: "runtime not registered".into(),
            })?;

        if !runtime.is_available() {
            return Err(InferenceError::RuntimeUnavailable {
                runtime: kind,
                reason: "runtime reports unavailable".into(),
            });
        }

        // Evict if at capacity, and reserve a slot for the new model.
        {
            let mut models = self.loaded_models.lock();
            while models.len() >= self.max_loaded {
                if let Some(evict_id) = models.keys().next().cloned() {
                    let evict_kind = models[&evict_id];
                    if let Some(rt) = self.runtimes.get(&evict_kind) {
                        if let Err(e) = rt.unload_model(&evict_id) {
                            tracing::warn!(
                                model = %evict_id,
                                error = %e,
                                "failed to unload model during eviction"
                            );
                            return Err(InferenceError::Other(format!(
                                "failed to unload model `{evict_id}` during eviction: {e}"
                            )));
                        }
                    }
                    models.remove(&evict_id);
                    tracing::info!(evicted = %evict_id, "evicted model to stay within budget");
                } else {
                    break;
                }
            }
            models.insert(model_id.to_string(), kind);
        }

        if let Err(e) = runtime.load_model(model_id, path) {
            self.loaded_models.lock().remove(model_id);
            return Err(e);
        }
        Ok(())
    }

    /// Unloads a model from its runtime and removes it from tracking.
    pub fn unload_model(&self, model_id: &str) -> Result<(), InferenceError> {
        let kind = {
            let models = self.loaded_models.lock();
            models.get(model_id).copied()
        };

        if let Some(kind) = kind {
            if let Some(runtime) = self.runtimes.get(&kind) {
                runtime.unload_model(model_id)?;
            }
            self.loaded_models.lock().remove(model_id);
        }
        Ok(())
    }

    /// Returns `true` if the given model is currently loaded.
    pub fn is_loaded(&self, model_id: &str) -> bool {
        self.loaded_models.lock().contains_key(model_id)
    }

    /// Runs inference on a loaded model, automatically dispatching to the
    /// correct runtime.
    pub fn infer(
        &self,
        model_id: &str,
        request: &InferenceRequest,
    ) -> Result<InferenceOutput, InferenceError> {
        let kind = {
            let models = self.loaded_models.lock();
            models.get(model_id).copied()
        };

        let kind =
            kind.ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        let runtime =
            self.runtimes.get(&kind).ok_or_else(|| InferenceError::RuntimeUnavailable {
                runtime: kind,
                reason: "runtime not registered".into(),
            })?;

        runtime.infer(model_id, request)
    }

    /// Computes embeddings for text using a loaded model, automatically
    /// dispatching to the correct runtime.
    pub fn embed(&self, model_id: &str, text: &str) -> Result<Vec<f32>, InferenceError> {
        let kind = {
            let models = self.loaded_models.lock();
            models.get(model_id).copied()
        };

        let kind =
            kind.ok_or_else(|| InferenceError::ModelNotLoaded { model_id: model_id.to_string() })?;

        let runtime =
            self.runtimes.get(&kind).ok_or_else(|| InferenceError::RuntimeUnavailable {
                runtime: kind,
                reason: "runtime not registered".into(),
            })?;

        runtime.embed(model_id, text)
    }

    /// Returns the list of available runtime kinds.
    pub fn available_runtimes(&self) -> Vec<InferenceRuntimeKind> {
        self.runtimes.keys().copied().collect()
    }

    /// Returns a snapshot of which models are currently loaded and their
    /// associated runtime kind.
    pub fn loaded_model_statuses(&self) -> Vec<(String, InferenceRuntimeKind)> {
        self.loaded_models.lock().iter().map(|(id, kind)| (id.clone(), *kind)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_all_runtimes() {
        let mgr = RuntimeManager::new(4);
        assert!(mgr.get_runtime(InferenceRuntimeKind::Candle).is_some());
        assert!(mgr.get_runtime(InferenceRuntimeKind::Onnx).is_some());
        assert!(mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).is_some());
    }

    #[test]
    fn available_runtimes_returns_three() {
        let mgr = RuntimeManager::new(4);
        let kinds = mgr.available_runtimes();
        assert_eq!(kinds.len(), 3);
        assert!(kinds.contains(&InferenceRuntimeKind::Candle));
        assert!(kinds.contains(&InferenceRuntimeKind::Onnx));
        assert!(kinds.contains(&InferenceRuntimeKind::LlamaCpp));
    }

    #[test]
    fn is_loaded_false_initially() {
        let mgr = RuntimeManager::new(4);
        assert!(!mgr.is_loaded("anything"));
    }

    #[test]
    fn load_missing_file_fails() {
        let mgr = RuntimeManager::new(4);
        let result = mgr.load_model(
            "missing",
            Path::new("/does/not/exist.gguf"),
            InferenceRuntimeKind::LlamaCpp,
        );
        assert!(result.is_err());
    }

    #[test]
    fn infer_unloaded_model_fails() {
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
    fn embed_unloaded_model_fails() {
        let mgr = RuntimeManager::new(4);
        let result = mgr.embed("nonexistent", "hello");
        assert!(result.is_err());
    }

    #[test]
    fn unload_nonexistent_is_ok() {
        let mgr = RuntimeManager::new(4);
        let result = mgr.unload_model("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn get_runtime_returns_correct_kind() {
        let mgr = RuntimeManager::new(4);
        let candle = mgr.get_runtime(InferenceRuntimeKind::Candle).unwrap();
        assert_eq!(candle.kind(), InferenceRuntimeKind::Candle);
        let onnx = mgr.get_runtime(InferenceRuntimeKind::Onnx).unwrap();
        assert_eq!(onnx.kind(), InferenceRuntimeKind::Onnx);
        let llama = mgr.get_runtime(InferenceRuntimeKind::LlamaCpp).unwrap();
        assert_eq!(llama.kind(), InferenceRuntimeKind::LlamaCpp);
    }

    // Stub-only tests: load/infer/embed with fake model files
    #[cfg(not(any(feature = "candle", feature = "llama-cpp", feature = "onnx")))]
    mod stub_tests {
        use super::*;
        use std::fs;
        use tempfile::tempdir;

        #[test]
        fn load_and_infer_round_trip() {
            let dir = tempdir().unwrap();
            let model_path = dir.path().join("model.gguf");
            fs::write(&model_path, b"fake").unwrap();

            let mgr = RuntimeManager::new(4);
            mgr.load_model("m1", &model_path, InferenceRuntimeKind::LlamaCpp).unwrap();
            assert!(mgr.is_loaded("m1"));

            let req = InferenceRequest { prompt: "Hi".into(), ..Default::default() };
            let out = mgr.infer("m1", &req).unwrap();
            assert!(out.text.contains("llama.cpp"));
        }

        #[test]
        fn load_and_embed_round_trip() {
            let dir = tempdir().unwrap();
            let model_path = dir.path().join("model.safetensors");
            fs::write(&model_path, b"fake").unwrap();

            let mgr = RuntimeManager::new(4);
            mgr.load_model("embed", &model_path, InferenceRuntimeKind::Candle).unwrap();

            let vec = mgr.embed("embed", "test text").unwrap();
            assert!(!vec.is_empty());
        }

        #[test]
        fn unload_removes_model() {
            let dir = tempdir().unwrap();
            let model_path = dir.path().join("model.onnx");
            fs::write(&model_path, b"fake").unwrap();

            let mgr = RuntimeManager::new(4);
            mgr.load_model("m1", &model_path, InferenceRuntimeKind::Onnx).unwrap();
            assert!(mgr.is_loaded("m1"));

            mgr.unload_model("m1").unwrap();
            assert!(!mgr.is_loaded("m1"));
        }

        #[test]
        fn eviction_when_at_capacity() {
            let dir = tempdir().unwrap();
            let p1 = dir.path().join("m1.gguf");
            let p2 = dir.path().join("m2.gguf");
            let p3 = dir.path().join("m3.gguf");
            fs::write(&p1, b"m1").unwrap();
            fs::write(&p2, b"m2").unwrap();
            fs::write(&p3, b"m3").unwrap();

            let mgr = RuntimeManager::new(2);
            mgr.load_model("m1", &p1, InferenceRuntimeKind::LlamaCpp).unwrap();
            mgr.load_model("m2", &p2, InferenceRuntimeKind::LlamaCpp).unwrap();
            // Third load should evict one
            mgr.load_model("m3", &p3, InferenceRuntimeKind::LlamaCpp).unwrap();

            let models = mgr.loaded_models.lock();
            assert_eq!(models.len(), 2);
            assert!(models.contains_key("m3"));
        }
    }
}
