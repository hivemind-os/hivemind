// Hide the console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::{Context, Result};
use hive_api::{build_router, AppState};
use hive_classification::DataClass;
use hive_core::{discover_paths, ensure_paths, load_config, AuditLogger, EventBus, NewAuditEntry};
use std::fs::{self, OpenOptions};
use std::sync::Arc;
use tokio::sync::Notify;
use tracing::{error, info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter, Layer};

/// Trigger macOS TCC permission prompts for Calendar and/or Contacts, then
/// exit.  This must be run from a process with a GUI session (e.g. spawned
/// by the desktop app) so macOS can actually display the permission dialog.
#[cfg(target_os = "macos")]
fn request_access_and_exit() -> Result<()> {
    use objc2_contacts::{CNAuthorizationStatus, CNContactStore, CNEntityType};
    use objc2_event_kit::{EKAuthorizationStatus, EKEntityType, EKEventStore};

    // Link CoreFoundation for CFRunLoop (needed to process the TCC dialog).
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFRunLoopGetMain() -> *mut std::ffi::c_void;
        fn CFRunLoopRunInMode(
            mode: *const std::ffi::c_void,
            seconds: f64,
            return_after_source_handled: u8,
        ) -> i32;
    }
    extern "C" {
        // kCFRunLoopDefaultMode is a CFStringRef global symbol.
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
    }

    /// Spin the main CFRunLoop briefly so macOS can process TCC XPC dialogs.
    fn pump_run_loop(seconds: f64) {
        unsafe {
            let _ = CFRunLoopGetMain();
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, seconds, 0);
        }
    }

    let args: Vec<String> = std::env::args().collect();
    let want_calendar = args.iter().any(|a| a == "--request-calendar-access");
    let want_contacts = args.iter().any(|a| a == "--request-contacts-access");
    let mut denied = false;

    if want_calendar {
        let status = unsafe { EKEventStore::authorizationStatusForEntityType(EKEntityType::Event) };
        #[allow(deprecated)]
        if status == EKAuthorizationStatus::Authorized
            || status == EKAuthorizationStatus::FullAccess
        {
            eprintln!("Calendar access: already granted");
        } else {
            eprintln!("Requesting calendar access...");
            let store = unsafe { EKEventStore::new() };
            let (tx, rx) = std::sync::mpsc::channel();
            let block = block2::RcBlock::new(
                move |granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {
                    let _ = tx.send(granted.as_bool());
                },
            );
            unsafe {
                store.requestFullAccessToEventsWithCompletion(&*block as *const _ as *mut _);
            }
            loop {
                pump_run_loop(0.5);
                match rx.try_recv() {
                    Ok(true) => {
                        eprintln!("Calendar access: granted");
                        break;
                    }
                    Ok(false) => {
                        eprintln!("Calendar access: denied");
                        denied = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                    Err(_) => {
                        eprintln!("Calendar access: channel closed");
                        denied = true;
                        break;
                    }
                }
            }
        }
    }

    if want_contacts {
        let status =
            unsafe { CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts) };
        if status == CNAuthorizationStatus::Authorized || status == CNAuthorizationStatus::Limited {
            eprintln!("Contacts access: already granted");
        } else {
            eprintln!("Requesting contacts access...");
            let store = unsafe { CNContactStore::new() };
            let (tx, rx) = std::sync::mpsc::channel();
            let block = block2::RcBlock::new(
                move |granted: objc2::runtime::Bool, _error: *mut objc2_foundation::NSError| {
                    let _ = tx.send(granted.as_bool());
                },
            );
            unsafe {
                store.requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block);
            }
            loop {
                pump_run_loop(0.5);
                match rx.try_recv() {
                    Ok(true) => {
                        eprintln!("Contacts access: granted");
                        break;
                    }
                    Ok(false) => {
                        eprintln!("Contacts access: denied");
                        denied = true;
                        break;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => continue,
                    Err(_) => {
                        eprintln!("Contacts access: channel closed");
                        denied = true;
                        break;
                    }
                }
            }
        }
    }

    // Exit code 2 signals "denied" (distinct from generic failure) so
    // callers can open System Settings as a fallback.
    if denied {
        std::process::exit(2);
    }

    Ok(())
}

fn main() -> Result<()> {
    // Handle --request-access before any other setup.
    // This mode triggers macOS TCC prompts for Calendar/Contacts, then exits.
    // Must be run from a GUI process (e.g. spawned by the desktop app) so
    // that macOS can display the permission dialog.
    if std::env::args()
        .any(|a| a == "--request-calendar-access" || a == "--request-contacts-access")
    {
        #[cfg(target_os = "macos")]
        return request_access_and_exit();

        #[cfg(not(target_os = "macos"))]
        {
            eprintln!("--request-*-access flags are only supported on macOS");
            return Ok(());
        }
    }

    let config = load_config().context("failed to load hivemind config")?;
    let paths = discover_paths().context("failed to discover hivemind paths")?;
    ensure_paths(&paths).context("failed to create hivemind runtime directories")?;

    let env_filter = EnvFilter::try_new(config.daemon.log_level.clone())
        .unwrap_or_else(|_| EnvFilter::new("info"));

    // Log to both stderr (for interactive use) and <hivemind_home>/daemon.log
    // (persisted when the daemon is spawned with stdout/stderr at /dev/null).
    let log_path = paths.hivemind_home.join("daemon.log");
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("failed to open daemon log {}", log_path.display()))?;

    let console_layer = fmt::layer().with_writer(std::io::stderr);
    let file_layer = fmt::layer().with_writer(log_file).with_ansi(false);

    // Per-service log ring buffer layer — shared with AppState so the API can
    // query captured logs.
    let service_log_collector = hive_core::ServiceLogCollector::new();

    // Apply the user-configured log level filter only to the console/file fmt
    // layers.  The ServiceLogCollector gets a permissive filter so it captures
    // all events emitted inside service spans regardless of the global level.
    let service_filter = EnvFilter::try_new("trace").unwrap_or_else(|_| EnvFilter::new("trace"));

    tracing_subscriber::registry()
        .with(console_layer.with_filter(env_filter))
        .with(
            file_layer.with_filter(
                EnvFilter::try_new(config.daemon.log_level.clone())
                    .unwrap_or_else(|_| EnvFilter::new("info")),
            ),
        )
        .with(service_log_collector.clone().with_filter(service_filter))
        .init();

    if !config.api.http_enabled {
        anyhow::bail!("http api is disabled in the current hivemind config");
    }

    let audit =
        AuditLogger::new(&paths.audit_log_path).context("failed to initialise audit log")?;
    audit
        .append(NewAuditEntry::new(
            "daemon",
            "daemon.start",
            "daemon",
            DataClass::Internal,
            format!(
                "starting hive-daemon v{} on {}",
                env!("CARGO_PKG_VERSION"),
                config.api.bind
            ),
            "success",
        ))
        .context("failed to write daemon startup audit entry")?;

    info!("hive-daemon v{} starting on {}", env!("CARGO_PKG_VERSION"), config.api.bind);

    // Bind the port BEFORE generating the auth token so that a second
    // daemon instance fails fast without overwriting the running daemon's
    // token in the OS keyring.
    let std_listener = std::net::TcpListener::bind(&config.api.bind)
        .with_context(|| format!("failed to bind {}", config.api.bind))?;
    std_listener.set_nonblocking(true).context("failed to set listener to non-blocking")?;

    // Write the actual bound address to a discovery file so clients can
    // find us when port 0 (dynamic) is used or when the config differs.
    let addr_file_path = paths.run_dir.join("daemon.addr");
    let local_addr = std_listener.local_addr().context("failed to get local address")?;
    fs::write(&addr_file_path, local_addr.to_string())
        .with_context(|| format!("failed to write addr file {}", addr_file_path.display()))?;

    // Now that we own the port, generate a fresh daemon auth token and
    // store it in the OS keyring.  Clients (CLI, desktop) will read it
    // from the keyring and attach it as a Bearer header to every API
    // request.
    let auth_token = hive_core::daemon_token::generate_and_store();

    // Build AppState (and its reqwest::blocking clients) BEFORE entering the
    // tokio runtime so the blocking clients' internal runtimes are not nested.
    let event_bus = EventBus::new(config.daemon.event_bus_capacity);
    let shutdown = Arc::new(Notify::new());
    let mut state = AppState::new(
        config.clone(),
        audit.clone(),
        event_bus.clone(),
        shutdown.clone(),
        auth_token,
    )
    .context("failed to initialise application state")?;
    state.set_service_log_collector(service_log_collector);

    fs::write(&paths.pid_file_path, std::process::id().to_string())
        .with_context(|| format!("failed to write pid file {}", paths.pid_file_path.display()))?;

    let rt = tokio::runtime::Runtime::new().context("failed to create tokio runtime")?;
    let serve_result = rt.block_on(async {
        // Start background services now that we're inside the runtime.
        state.start_background().await;
        let router = build_router(state.clone());

        let ctrl_c_shutdown = shutdown.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                warn!("ctrl-c received, shutting daemon down");
                ctrl_c_shutdown.notify_waiters();
            }
        });
        #[cfg(unix)]
        {
            let term_shutdown = shutdown.clone();
            tokio::spawn(async move {
                use tokio::signal::unix::{signal, SignalKind};
                match signal(SignalKind::terminate()) {
                    Ok(mut stream) => {
                        stream.recv().await;
                        warn!("sigterm received, shutting daemon down");
                        term_shutdown.notify_waiters();
                    }
                    Err(e) => warn!("failed to install SIGTERM handler: {e}"),
                }
            });
        }

        let listener = tokio::net::TcpListener::from_std(std_listener)
            .context("failed to convert listener to async")?;

        info!("HiveMind OS daemon listening on http://{}", local_addr);
        info!("PID {}", std::process::id());

        axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                shutdown.notified().await;
            })
            .await
            .context("daemon server exited with an error")?;

        // Stop background services so spawned tasks complete promptly.
        state.shutdown().await;

        Ok::<(), anyhow::Error>(())
    });

    // Shut down the tokio runtime with a timeout so that lingering
    // spawn_blocking tasks (e.g. IMAP IDLE) don't hang the process.
    // The graceful application shutdown above already stopped all services;
    // any remaining blocking threads are orphaned I/O waits.
    rt.shutdown_timeout(std::time::Duration::from_secs(3));

    let shutdown_result = audit.append(NewAuditEntry::new(
        "daemon",
        "daemon.stop",
        "daemon",
        DataClass::Internal,
        "daemon shutdown completed",
        if serve_result.is_ok() { "success" } else { "error" },
    ));

    if let Err(error) = shutdown_result {
        error!("failed to write daemon shutdown audit entry: {error:#}");
    }

    let _ = fs::remove_file(&paths.pid_file_path);
    let _ = fs::remove_file(&addr_file_path);

    // Remove the daemon auth token from the OS keyring so stale tokens
    // don't linger after shutdown.
    hive_core::daemon_token::clear();

    serve_result
}
