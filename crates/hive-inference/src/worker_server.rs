use std::io::{BufRead, Write};
use std::sync::Arc;

use crate::ipc::{IpcMethod, IpcRequest, IpcResponse, IpcResult};
use crate::runtime::{InferenceError, InferenceRuntime};

/// Runs the worker-side event loop: reads [`IpcRequest`]s from stdin,
/// dispatches to the given [`InferenceRuntime`], and writes
/// [`IpcResponse`]s to stdout.
///
/// This function blocks forever (or until stdin is closed / the process
/// is killed). It is intended to be the `main` of the
/// `hive-runtime-worker` binary.
///
/// All tracing / logging MUST go to stderr so it does not interfere
/// with the JSON protocol on stdout.
pub fn run_worker_loop(runtime: Arc<dyn InferenceRuntime>) {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let reader = stdin.lock();
    let mut writer = stdout.lock();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("failed to read stdin: {e}");
                break;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let request: IpcRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                // Can't even parse the id, so use 0.
                let resp = IpcResponse::error(0, "parse_error", format!("invalid request: {e}"));
                if let Err(e) = write_response(&mut writer, &resp) {
                    tracing::error!("failed to write parse_error response: {e}");
                    break;
                }
                continue;
            }
        };

        let response = dispatch(&runtime, &request);

        if let Err(e) = write_response(&mut writer, &response) {
            tracing::error!(id = request.id, "failed to write response: {e}");
            break;
        }
    }
}

fn write_response(writer: &mut impl Write, response: &IpcResponse) -> std::io::Result<()> {
    let json = serde_json::to_string(response).expect("IpcResponse must be serializable");
    writer.write_all(json.as_bytes())?;
    writer.write_all(b"\n")?;
    writer.flush()
}

fn dispatch(runtime: &Arc<dyn InferenceRuntime>, request: &IpcRequest) -> IpcResponse {
    let id = request.id;

    // Catch panics so a misbehaving runtime doesn't kill the worker
    // without sending an error response.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch_inner(runtime, id, &request.method)
    }));

    match result {
        Ok(resp) => resp,
        Err(panic) => {
            let msg = if let Some(s) = panic.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            tracing::error!(id, panic = %msg, "runtime panicked");
            IpcResponse::error(id, "panic", format!("runtime panicked: {msg}"))
        }
    }
}

fn dispatch_inner(runtime: &Arc<dyn InferenceRuntime>, id: u64, method: &IpcMethod) -> IpcResponse {
    match method {
        IpcMethod::RuntimeKind => IpcResponse::result(id, IpcResult::RuntimeKind(runtime.kind())),
        IpcMethod::RuntimeIsAvailable => {
            IpcResponse::result(id, IpcResult::Bool(runtime.is_available()))
        }
        IpcMethod::RuntimeInfo => IpcResponse::result(id, IpcResult::RuntimeInfo(runtime.info())),
        IpcMethod::RuntimeFormats => {
            IpcResponse::result(id, IpcResult::Formats(runtime.supported_formats()))
        }
        IpcMethod::ModelLoad { model_id, model_path } => {
            match runtime.load_model(model_id, model_path) {
                Ok(()) => IpcResponse::ok(id),
                Err(e) => inference_error_to_response(id, &e),
            }
        }
        IpcMethod::ModelUnload { model_id } => match runtime.unload_model(model_id) {
            Ok(()) => IpcResponse::ok(id),
            Err(e) => inference_error_to_response(id, &e),
        },
        IpcMethod::ModelIsLoaded { model_id } => {
            IpcResponse::result(id, IpcResult::Bool(runtime.is_model_loaded(model_id)))
        }
        IpcMethod::ModelInfer { model_id, request, .. } => match runtime.infer(model_id, request) {
            Ok(output) => IpcResponse::result(id, IpcResult::InferenceOutput(output)),
            Err(e) => inference_error_to_response(id, &e),
        },
        IpcMethod::ModelEmbed { model_id, text } => match runtime.embed(model_id, text) {
            Ok(embeddings) => IpcResponse::result(id, IpcResult::Embeddings(embeddings)),
            Err(e) => inference_error_to_response(id, &e),
        },
    }
}

fn inference_error_to_response(id: u64, err: &InferenceError) -> IpcResponse {
    let code = match err {
        InferenceError::ModelNotLoaded { .. } => "model_not_loaded",
        InferenceError::RuntimeUnavailable { .. } => "runtime_unavailable",
        InferenceError::LoadFailed(_) => "load_failed",
        InferenceError::InferenceFailed(_) => "inference_failed",
        InferenceError::ModelFileNotFound(_) => "model_file_not_found",
        InferenceError::UnsupportedFormat { .. } => "unsupported_format",
        InferenceError::WorkerCrashed(_) => "worker_crashed",
        InferenceError::Timeout { .. } => "timeout",
        InferenceError::Other(_) => "other",
    };
    IpcResponse::error(id, code, err.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc::IpcPayload;
    use crate::runtime::{CandleRuntime, InferenceRequest};
    use hive_core::InferenceRuntimeKind;

    fn test_runtime() -> Arc<dyn InferenceRuntime> {
        Arc::new(CandleRuntime::new())
    }

    #[test]
    fn dispatch_runtime_kind() {
        let rt = test_runtime();
        let req = IpcRequest { id: 1, method: IpcMethod::RuntimeKind };
        let resp = dispatch(&rt, &req);
        assert_eq!(resp.id, 1);
        match resp.payload {
            IpcPayload::Result(IpcResult::RuntimeKind(k)) => {
                assert_eq!(k, InferenceRuntimeKind::Candle);
            }
            other => panic!("expected RuntimeKind, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_is_available() {
        let rt = test_runtime();
        let req = IpcRequest { id: 2, method: IpcMethod::RuntimeIsAvailable };
        let resp = dispatch(&rt, &req);
        match resp.payload {
            IpcPayload::Result(IpcResult::Bool(true)) => {}
            other => panic!("expected Bool(true), got {other:?}"),
        }
    }

    #[test]
    fn dispatch_model_not_loaded_error() {
        let rt = test_runtime();
        let req = IpcRequest {
            id: 3,
            method: IpcMethod::ModelInfer {
                model_id: "nonexistent".to_string(),
                request: InferenceRequest::default(),
                attachments: vec![],
            },
        };
        let resp = dispatch(&rt, &req);
        match resp.payload {
            IpcPayload::Error(e) => {
                assert_eq!(e.code, "model_not_loaded");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_load_missing_file() {
        let rt = test_runtime();
        let req = IpcRequest {
            id: 4,
            method: IpcMethod::ModelLoad {
                model_id: "test".to_string(),
                model_path: "/nonexistent/model.bin".into(),
            },
        };
        let resp = dispatch(&rt, &req);
        match resp.payload {
            IpcPayload::Error(e) => {
                assert_eq!(e.code, "model_file_not_found");
            }
            other => panic!("expected error, got {other:?}"),
        }
    }
}
