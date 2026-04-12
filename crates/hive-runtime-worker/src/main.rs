use std::sync::Arc;

use clap::Parser;
use hive_inference::runtime::{CandleRuntime, InferenceRuntime, LlamaCppRuntime, OnnxRuntime};
use hive_inference::worker_server::run_worker_loop;

/// Suppress the Windows CRT debug assertion / abort dialog that blocks
/// the process waiting for user input.  In an isolated worker process
/// we want crashes to terminate immediately so the parent can detect
/// and report the failure.
#[cfg(target_os = "windows")]
fn suppress_crt_dialogs() {
    unsafe {
        extern "C" {
            fn _set_abort_behavior(flags: u32, mask: u32) -> u32;
        }
        extern "system" {
            fn SetErrorMode(mode: u32) -> u32;
        }
        // Disable the abort message box (_WRITE_ABORT_MSG = 0x1)
        // and the call to Dr. Watson (_CALL_REPORTFAULT = 0x2).
        _set_abort_behavior(0, 0x1 | 0x2);
        // SEM_FAILCRITICALERRORS (0x1) | SEM_NOGPFAULTERRORBOX (0x2)
        SetErrorMode(0x1 | 0x2);
    }
}

#[cfg(not(target_os = "windows"))]
fn suppress_crt_dialogs() {}

#[derive(Parser)]
#[command(name = "hive-runtime-worker", about = "Isolated inference runtime worker")]
struct Cli {
    /// Which runtime to host: candle, onnx, or llama-cpp
    #[arg(long)]
    runtime: String,
}

fn main() {
    suppress_crt_dialogs();

    // Tracing goes to stderr only — stdout is reserved for the IPC protocol.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cli = Cli::parse();

    // Install a panic hook that logs to stderr before aborting.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("PANIC in runtime worker: {info}");
        default_hook(info);
    }));

    let runtime: Arc<dyn InferenceRuntime> = match cli.runtime.as_str() {
        "candle" => {
            tracing::info!("starting Candle runtime worker");
            Arc::new(CandleRuntime::new())
        }
        "onnx" => {
            tracing::info!("starting ONNX runtime worker");
            Arc::new(OnnxRuntime::new())
        }
        "llama-cpp" => {
            tracing::info!("starting llama.cpp runtime worker");
            Arc::new(LlamaCppRuntime::new())
        }
        other => {
            tracing::error!(runtime = other, "unknown runtime kind");
            std::process::exit(1);
        }
    };

    tracing::info!(runtime = cli.runtime.as_str(), "worker ready, entering event loop");
    run_worker_loop(runtime);
    tracing::info!("worker exiting");
}
