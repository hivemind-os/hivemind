use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::runtime::{InferenceOutput, InferenceRequest, RuntimeInfo};
use hive_core::InferenceRuntimeKind;

// ---------------------------------------------------------------------------
// Attachments (for multimodal inference)
// ---------------------------------------------------------------------------

/// A file attachment sent alongside an inference request.
/// Binary data is never inlined — only the file path is transmitted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Attachment {
    pub path: PathBuf,
    pub media_type: String,
}

// ---------------------------------------------------------------------------
// IPC Request
// ---------------------------------------------------------------------------

/// A request sent from the daemon to a runtime worker process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    #[serde(flatten)]
    pub method: IpcMethod,
}

/// The set of methods a runtime worker can handle, with their parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum IpcMethod {
    /// Returns the runtime kind.
    RuntimeKind,
    /// Checks if the runtime is available on this system.
    RuntimeIsAvailable,
    /// Returns runtime info and diagnostics.
    RuntimeInfo,
    /// Returns supported file extensions.
    RuntimeFormats,
    /// Loads a model from a local path.
    ModelLoad { model_id: String, model_path: PathBuf },
    /// Unloads a previously loaded model.
    ModelUnload { model_id: String },
    /// Checks if a model is currently loaded.
    ModelIsLoaded { model_id: String },
    /// Runs inference against a loaded model.
    ModelInfer {
        model_id: String,
        request: InferenceRequest,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<Attachment>,
    },
    /// Computes embeddings for text input.
    ModelEmbed { model_id: String, text: String },
}

// ---------------------------------------------------------------------------
// IPC Response
// ---------------------------------------------------------------------------

/// A response sent from a runtime worker back to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    #[serde(flatten)]
    pub payload: IpcPayload,
}

/// The result of an IPC call — either success or error.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcPayload {
    Result(IpcResult),
    Error(IpcError),
}

/// Successful results, tagged by the kind of data returned.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum IpcResult {
    RuntimeKind(InferenceRuntimeKind),
    Bool(bool),
    RuntimeInfo(RuntimeInfo),
    Formats(Vec<String>),
    InferenceOutput(InferenceOutput),
    Embeddings(Vec<f32>),
    Ok,
}

/// An error returned by the worker process.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl IpcResponse {
    pub fn ok(id: u64) -> Self {
        Self { id, payload: IpcPayload::Result(IpcResult::Ok) }
    }

    pub fn result(id: u64, result: IpcResult) -> Self {
        Self { id, payload: IpcPayload::Result(result) }
    }

    pub fn error(id: u64, code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            id,
            payload: IpcPayload::Error(IpcError { code: code.into(), message: message.into() }),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_through_json() {
        let req = IpcRequest {
            id: 42,
            method: IpcMethod::ModelInfer {
                model_id: "llama-7b".to_string(),
                request: InferenceRequest { prompt: "Hello".to_string(), ..Default::default() },
                attachments: vec![Attachment {
                    path: PathBuf::from("/tmp/img.png"),
                    media_type: "image/png".to_string(),
                }],
            },
        };

        let json = serde_json::to_string(&req).unwrap();
        let decoded: IpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 42);
        match &decoded.method {
            IpcMethod::ModelInfer { model_id, attachments, .. } => {
                assert_eq!(model_id, "llama-7b");
                assert_eq!(attachments.len(), 1);
                assert_eq!(attachments[0].media_type, "image/png");
            }
            other => panic!("expected ModelInfer, got {other:?}"),
        }
    }

    #[test]
    fn response_ok_round_trips() {
        let resp = IpcResponse::ok(1);
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: IpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 1);
        assert!(matches!(decoded.payload, IpcPayload::Result(IpcResult::Ok)));
    }

    #[test]
    fn response_error_round_trips() {
        let resp = IpcResponse::error(2, "model_not_loaded", "model not loaded: test");
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: IpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 2);
        match decoded.payload {
            IpcPayload::Error(e) => {
                assert_eq!(e.code, "model_not_loaded");
                assert!(e.message.contains("test"));
            }
            _ => panic!("expected error"),
        }
    }

    #[test]
    fn response_inference_output_round_trips() {
        let resp = IpcResponse::result(
            3,
            IpcResult::InferenceOutput(InferenceOutput {
                text: "Hello!".to_string(),
                tokens_used: 5,
                finish_reason: crate::runtime::FinishReason::Stop,
            }),
        );
        let json = serde_json::to_string(&resp).unwrap();
        let decoded: IpcResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.id, 3);
    }

    #[test]
    fn request_without_attachments_omits_field() {
        let req = IpcRequest {
            id: 1,
            method: IpcMethod::ModelInfer {
                model_id: "m1".to_string(),
                request: InferenceRequest::default(),
                attachments: vec![],
            },
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(!json.contains("attachments"));
    }

    #[test]
    fn simple_methods_serialize_without_params() {
        let req = IpcRequest { id: 1, method: IpcMethod::RuntimeKind };
        let json = serde_json::to_string(&req).unwrap();
        let decoded: IpcRequest = serde_json::from_str(&json).unwrap();
        assert!(matches!(decoded.method, IpcMethod::RuntimeKind));
    }
}
