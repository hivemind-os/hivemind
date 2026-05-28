//! # hive-inference
//!
//! Local model inference runtime support for HiveMind OS. This crate covers
//! hardware detection, model discovery and downloads, registry persistence, runtime
//! execution, and worker-process coordination for on-device inference backends.
//!
//! ## Key exports
//!
//! - [`HubClient`] and [`DownloadProgress`] — discover and fetch model artifacts.
//! - [`LocalModelRegistry`] and [`SqliteModelRegistry`] — persist installed model metadata.
//! - [`InferenceRuntime`], [`InferenceRequest`], and [`InferenceOutput`] — backend runtime contract.
//! - [`RuntimeManager`] and [`RuntimeWorkerProxy`] — manage loaded runtimes and worker processes.
//!
//! ## Crate relationships
//!
//! Depends on `hive-core`, `hive-contracts`, and `hive-classification`, and is used by
//! `hive-model`, `hive-chat`, and `hive-api` to supply local-model capabilities.
//!
//! ## Usage notes
//!
//! Enable the `candle`, `llama-cpp`, or `onnx` features to compile specific local inference backends.

pub mod defaults;
pub mod embedding;
pub mod hardware;
pub mod hub;
pub mod ipc;
pub mod registry;
pub mod runtime;
pub mod runtime_manager;
pub mod worker_proxy;
pub mod worker_server;

pub mod gpu;

#[cfg(feature = "candle")]
pub mod runtime_candle;
#[cfg(feature = "llama-cpp")]
pub mod runtime_llama;
#[cfg(feature = "onnx")]
pub mod runtime_onnx;

pub use defaults::{
    default_chat_filename, default_chat_model_id, default_chat_repo, default_embedding_dimension,
    default_embedding_filename, default_embedding_model_id, default_embedding_repo,
};
pub use hardware::{current_resource_usage, detect_hardware};
pub use hive_contracts::{
    CpuInfo, GpuInfo, GpuVendor, HardwareInfo, MemoryInfo, RuntimeResourceUsage,
};
pub use hub::{
    infer_runtime, DownloadProgress, HubClient, HubFileInfo, HubModelInfo, HubSearchRequest,
    HubSearchResult,
};
pub use registry::{
    InferenceParams, InstalledModel, LocalModelRegistry, ModelCapabilities, ModelRegistryStore,
    ModelStatus, RegistryError, SqliteModelRegistry,
};
pub use gpu::{estimate_layer_count, recommend_gpu_layers};
pub use runtime::{
    ChatMessage, InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime,
    ModelLoadOptions, RuntimeInfo,
};
pub use runtime_manager::RuntimeManager;
pub use worker_proxy::{RuntimeWorkerProxy, WorkerConfig, WorkerState};
