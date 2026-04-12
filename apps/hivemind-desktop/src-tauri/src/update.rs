use tauri::{AppHandle, Emitter, Manager};

/// Spawn a background timer that periodically asks the frontend to check for
/// updates by emitting an `update:check` event. The actual version comparison
/// and download are handled by the `@tauri-apps/plugin-updater` JS API in the
/// frontend, keeping all UI/progress logic in one place.
pub fn spawn_update_timer(app: &AppHandle) {
    // Only run the timer when the updater plugin is actually loaded.
    let updater_enabled = app
        .config()
        .plugins
        .0
        .get("updater")
        .and_then(|v| v.get("pubkey"))
        .and_then(|v| v.as_str())
        .is_some_and(|k| !k.is_empty());
    if !updater_enabled {
        return;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        // Wait before the first check so the UI is fully loaded.
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        loop {
            let _ = handle.emit("update:check", ());
            // Re-check every 6 hours.
            tokio::time::sleep(std::time::Duration::from_secs(6 * 60 * 60)).await;
        }
    });
}

/// Emit `update:check` once so the frontend performs an immediate check.
/// Called from the system tray "Check for Updates" menu item.
pub fn trigger_update_check(app: &AppHandle) {
    let _ = app.emit("update:check", ());
    // Show/focus the main window so the user sees the result.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
