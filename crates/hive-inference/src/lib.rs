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
pub use runtime::{
    ChatMessage, InferenceError, InferenceOutput, InferenceRequest, InferenceRuntime, RuntimeInfo,
};
pub use runtime_manager::RuntimeManager;
pub use worker_proxy::{RuntimeWorkerProxy, WorkerConfig, WorkerState};
