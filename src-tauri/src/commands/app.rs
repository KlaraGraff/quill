use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

use crate::error::{AppError, AppResult};
use crate::resolve_log_dir;

#[derive(Debug, serde::Serialize)]
pub struct BuildInfo {
    pub version: String,
    pub upstream_baseline: String,
    pub commit: String,
    pub built_at: String,
    pub channel: String,
    pub bundle_identifier: String,
    pub repository: String,
    pub upstream_repository: String,
}

#[tauri::command]
pub fn app_build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        upstream_baseline: env!("QUILL_UPSTREAM_BASELINE").to_string(),
        commit: env!("QUILL_BUILD_COMMIT").to_string(),
        built_at: env!("QUILL_BUILD_DATE").to_string(),
        channel: env!("QUILL_BUILD_CHANNEL").to_string(),
        bundle_identifier: "com.klaragraff.quill".to_string(),
        repository: "https://github.com/KlaraGraff/quill".to_string(),
        upstream_repository: "https://github.com/yicheng47/quill".to_string(),
    }
}

/// Called by the frontend after React has mounted and painted its first frame.
/// Shows the main window — the window starts hidden so the user sees the dock
/// bounce → fully-rendered window instead of a beach ball over a blank webview.
#[tauri::command]
pub fn app_ready(app: AppHandle) -> AppResult<()> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| AppError::Other("main window not found".into()))?;
    window.show().map_err(|e| AppError::Other(e.to_string()))?;
    window
        .set_focus()
        .map_err(|e| AppError::Other(e.to_string()))?;
    Ok(())
}

/// Reveal the per-user app log directory in the OS file manager.
#[tauri::command]
pub fn reveal_logs(app: AppHandle) -> AppResult<()> {
    let log_dir = resolve_log_dir();
    app.opener()
        .open_path(log_dir.to_string_lossy(), None::<&str>)
        .map_err(|e| AppError::Other(format!("open log dir: {e}")))?;
    Ok(())
}
