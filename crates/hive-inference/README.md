# hive-inference

Local model inference runtimes and HuggingFace Hub integration for [HiveMind OS](../../README.md), a cross-platform, privacy-aware desktop AI agent.

## Modules

| Module | Purpose | Key Types |
|--------|---------|-----------|
| **`hardware`** | Hardware detection | `detect_hardware()`, `current_resource_usage()`, `HardwareInfo`, `GpuInfo`, `MemoryInfo` |
| **`hub`** | HuggingFace Hub client | `HubClient`, `HubModelInfo`, `HubSearchRequest`, `DownloadProgress` |
| **`registry`** | Local model registry | `LocalModelRegistry`, `InstalledModel`, `ModelCapabilities`, `ModelStatus` |
| **`runtime`** | Inference execution | `InferenceRuntime` trait, `RuntimeManager`, `InferenceRequest`, `InferenceOutput` |

### hardware

Detects CPU/GPU capabilities and memory constraints. Used to determine optimal runtime selection and resource budgets.

### hub

Async HTTP client for the HuggingFace Hub API. Supports model discovery, search, and file download with progress reporting.

### registry

SQLite-based persistence for installed model metadata — model ID, repository, filename, and capabilities. Tracks model status across sessions.

### runtime

Defines the `InferenceRuntime` trait and the `RuntimeManager`, which maintains an LRU cache of loaded models with memory management and a configurable memory ceiling.

## Feature Flags

Inference backends are feature-gated. Enable the ones you need in `Cargo.toml`:

| Feature | Backend | Crate Dependencies |
|---------|---------|-------------------|
| `candle` | Pure Rust inference via `runtime_candle` | `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers` |
| `llama-cpp` | C++ backend via `runtime_llama` | `llama-cpp-2`, `encoding_rs` |
| `onnx` | Cross-platform via `runtime_onnx` | `ort`, `tokenizers` |

## Dependencies

**Workspace (internal):**
- `hive-core` — shared core types
- `hive-contracts` — trait definitions and interfaces
- `hive-classification` — model classification utilities

**External:**
- `tokio` — async runtime
- `reqwest` — HTTP client (Hub downloads)
- `rusqlite` — SQLite storage (registry)
- `parking_lot` — synchronization primitives
- `sha2` — model file integrity checks
- `thiserror` — error types

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌──────────┐
│  HubClient  │────▶│   Registry   │────▶│ Runtime  │
│  (download) │     │  (SQLite)    │     │ Manager  │
└─────────────┘     └──────────────┘     └──────────┘
                                              │
                          ┌───────────────────┼───────────────────┐
                          │                   │                   │
                    ┌─────┴─────┐     ┌───────┴──────┐   ┌───────┴──────┐
                    │  Candle   │     │  llama.cpp   │   │     ONNX     │
                    │ (feature) │     │  (feature)   │   │  (feature)   │
                    └───────────┘     └──────────────┘   └──────────────┘
```

- **HubClient** discovers and downloads models from HuggingFace Hub.
- **Registry** stores model metadata in SQLite so models survive restarts.
- **RuntimeManager** loads models into an LRU cache with a configurable memory ceiling and dispatches inference requests to the appropriate feature-gated backend.
- **Hardware detection** informs runtime selection based on available CPU/GPU resources.
