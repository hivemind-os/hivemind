use tauri::image::Image as TauriImage;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Wry};

enum DaemonAction {
    Start,
    Stop,
    Restart,
}

/// Return the icon to use for the system tray.
///
/// On macOS we use a dedicated monochrome icon so it renders cleanly as a
/// menu-bar template image. On other platforms we reuse the app window icon.
fn tray_icon(_app: &AppHandle) -> tauri::image::Image<'static> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(img) = TauriImage::from_bytes(include_bytes!("../icons/tray-icon@2x.png")) {
            return img;
        }
    }
    // Full-color icon for Windows/Linux tray, and macOS fallback
    TauriImage::from_bytes(include_bytes!("../icons/icon.png"))
        .expect("embedded icon.png must be valid")
}

/// Set up the system tray icon (Windows) / menu bar status item (macOS).
///
/// The tray provides:
/// - "Open HiveMind OS" — shows/focuses the main window
/// - "Daemon: ..." — read-only status line updated periodically
/// - "Start Daemon" / "Stop Daemon" / "Restart Daemon" — daemon lifecycle
/// - "Check for Updates" — triggers an immediate update check
/// - "Quit HiveMind OS" — exits the application
pub fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let show = MenuItemBuilder::with_id("show", "Open HiveMind OS").build(app)?;
    let daemon_status = MenuItemBuilder::with_id("daemon_status", "Daemon: checking...")
        .enabled(false)
        .build(app)?;
    let separator1 = PredefinedMenuItem::separator(app)?;
    let start_daemon = MenuItemBuilder::with_id("start_daemon", "Start Daemon").build(app)?;
    let stop_daemon =
        MenuItemBuilder::with_id("stop_daemon", "Stop Daemon").enabled(false).build(app)?;
    let restart_daemon =
        MenuItemBuilder::with_id("restart_daemon", "Restart Daemon").enabled(false).build(app)?;
    let separator2 = PredefinedMenuItem::separator(app)?;
    let check_update = MenuItemBuilder::with_id("check_update", "Check for Updates").build(app)?;
    let separator3 = PredefinedMenuItem::separator(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit HiveMind OS").build(app)?;

    let menu = MenuBuilder::new(app)
        .items(&[
            &show,
            &daemon_status,
            &separator1,
            &start_daemon,
            &stop_daemon,
            &restart_daemon,
            &separator2,
            &check_update,
            &separator3,
            &quit,
        ])
        .build()?;

    // Clone menu items for the on_menu_event closure.
    let ev_status = daemon_status.clone();
    let ev_start = start_daemon.clone();
    let ev_stop = stop_daemon.clone();
    let ev_restart = restart_daemon.clone();

    #[allow(unused_variables)]
    let tray = TrayIconBuilder::with_id("main")
        .icon(tray_icon(app))
        .menu(&menu)
        .tooltip("HiveMind OS")
        .on_menu_event(move |app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            "check_update" => crate::update::trigger_update_check(app),
            "quit" => app.exit(0),
            id @ ("start_daemon" | "stop_daemon" | "restart_daemon") => {
                let action = match id {
                    "start_daemon" => DaemonAction::Start,
                    "stop_daemon" => DaemonAction::Stop,
                    _ => DaemonAction::Restart,
                };
                spawn_daemon_action(
                    ev_status.clone(),
                    ev_start.clone(),
                    ev_stop.clone(),
                    ev_restart.clone(),
                    action,
                );
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        })
        .build(app)?;

    // On macOS, use the icon as a template so it adapts to the menu bar
    // appearance (light/dark) automatically.
    #[cfg(target_os = "macos")]
    {
        let _ = tray.set_icon_as_template(true);
    }

    // Spawn a background poller that updates the daemon status and control
    // item states every 10 seconds.
    tauri::async_runtime::spawn(async move {
        loop {
            let running = check_daemon_status().await;
            update_daemon_menu_state(
                &daemon_status,
                &start_daemon,
                &stop_daemon,
                &restart_daemon,
                &running,
            );
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });

    Ok(())
}

/// Register a window-close handler that hides the window instead of quitting,
/// so the tray icon remains active and the daemon keeps running.
/// When `update_installing` is set, the handler lets the close proceed so the
/// NSIS installer can shut down the process and replace the binary.
pub fn setup_close_to_tray(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let win = window.clone();
        let state = app.state::<super::AppState>();
        let updating = state.update_installing.clone();
        window.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if updating.load(std::sync::atomic::Ordering::SeqCst) {
                    // Let the window close so the NSIS installer can proceed.
                    return;
                }
                api.prevent_close();
                let _ = win.hide();
            }
        });
    }
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

// ---------------------------------------------------------------------------
// Daemon menu helpers
// ---------------------------------------------------------------------------

/// Update the daemon status text and enable/disable the Start/Stop/Restart
/// menu items based on whether the daemon is currently reachable.
fn update_daemon_menu_state(
    status_item: &MenuItem<Wry>,
    start_item: &MenuItem<Wry>,
    stop_item: &MenuItem<Wry>,
    restart_item: &MenuItem<Wry>,
    running: &Option<String>,
) {
    let label = match running {
        Some(addr) => format!("Daemon: running on {addr}"),
        None => "Daemon: stopped".to_string(),
    };
    let _ = status_item.set_text(&label);
    let is_running = running.is_some();
    let _ = start_item.set_enabled(!is_running);
    let _ = stop_item.set_enabled(is_running);
    let _ = restart_item.set_enabled(is_running);
}

/// Spawn a background task that performs the given daemon action and then
/// refreshes the tray menu state.
fn spawn_daemon_action(
    status_item: MenuItem<Wry>,
    start_item: MenuItem<Wry>,
    stop_item: MenuItem<Wry>,
    restart_item: MenuItem<Wry>,
    action: DaemonAction,
) {
    // Disable all daemon controls and show an in-progress label.
    let _ = start_item.set_enabled(false);
    let _ = stop_item.set_enabled(false);
    let _ = restart_item.set_enabled(false);
    let progress_text = match action {
        DaemonAction::Start => "Daemon: starting...",
        DaemonAction::Stop => "Daemon: stopping...",
        DaemonAction::Restart => "Daemon: restarting...",
    };
    let _ = status_item.set_text(progress_text);

    tauri::async_runtime::spawn(async move {
        let result = tauri::async_runtime::spawn_blocking(move || match action {
            DaemonAction::Start => do_daemon_start(),
            DaemonAction::Stop => do_daemon_stop(),
            DaemonAction::Restart => do_daemon_restart(),
        })
        .await;

        if let Ok(Err(e)) = &result {
            tracing::warn!("tray daemon action failed: {e:#}");
        }

        // Refresh the menu to reflect the actual daemon state.
        let running = check_daemon_status().await;
        update_daemon_menu_state(&status_item, &start_item, &stop_item, &restart_item, &running);
    });
}

fn do_daemon_start() -> anyhow::Result<()> {
    let url = hive_core::daemon_url(None)?;
    hive_core::daemon_start(&url, None)?;
    Ok(())
}

fn do_daemon_stop() -> anyhow::Result<()> {
    let url = hive_core::daemon_url(None)?;
    hive_core::daemon_stop(&url)?;
    // Wait for the daemon to fully shut down.
    for _ in 0..25 {
        std::thread::sleep(std::time::Duration::from_millis(200));
        if hive_core::daemon_status(&url).is_err() {
            return Ok(());
        }
    }
    Ok(())
}

fn do_daemon_restart() -> anyhow::Result<()> {
    let url = hive_core::daemon_url(None)?;
    // Stop if currently running.
    if hive_core::daemon_status(&url).is_ok() {
        hive_core::daemon_stop(&url)?;
        for _ in 0..25 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if hive_core::daemon_status(&url).is_err() {
                break;
            }
        }
    }
    // Brief pause so the daemon can finish cleanup.
    std::thread::sleep(std::time::Duration::from_millis(500));
    // Re-resolve URL in case the address file changed during shutdown.
    let url = hive_core::daemon_url(None)?;
    hive_core::daemon_start(&url, None)?;
    Ok(())
}

/// Check whether the daemon is reachable and return its bound address.
async fn check_daemon_status() -> Option<String> {
    let url = hive_core::daemon_url(None).ok()?;
    let resp = reqwest::Client::new()
        .get(format!("{url}/api/v1/daemon/status"))
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .ok()?;
    if resp.status().is_success() {
        // Strip the "http://" prefix for a cleaner display
        let display = url.strip_prefix("http://").unwrap_or(&url);
        Some(display.to_string())
    } else {
        None
    }
}
