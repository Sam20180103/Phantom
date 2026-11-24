use std::path::PathBuf;
use tauri::Manager;

fn root(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ai_session_load(app: tauri::AppHandle, id: String) -> Result<Option<String>, String> {
    let root = root(&app)?;
    tauri::async_runtime::spawn_blocking(move || phantom_core::ai_sessions::load(&root, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn ai_session_save(
    app: tauri::AppHandle,
    id: String,
    content: String,
    migrate: bool,
    durable: bool,
) -> Result<(), String> {
    let root = root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        phantom_core::ai_sessions::save(&root, &id, &content, migrate, durable)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn ai_session_delete(app: tauri::AppHandle, id: String) -> Result<(), String> {
    let root = root(&app)?;
    tauri::async_runtime::spawn_blocking(move || phantom_core::ai_sessions::delete(&root, &id))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn ai_session_finish_migration(
    app: tauri::AppHandle,
    ids: Vec<String>,
) -> Result<bool, String> {
    let root = root(&app)?;
    tauri::async_runtime::spawn_blocking(move || phantom_core::ai_sessions::finish(&root, &ids))
        .await
        .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn ai_session_prune_backup(
    app: tauri::AppHandle,
    max_age_secs: u64,
) -> Result<bool, String> {
    let root = root(&app)?;
    tauri::async_runtime::spawn_blocking(move || {
        phantom_core::ai_sessions::prune(&root, max_age_secs)
    })
    .await
    .map_err(|e| e.to_string())?
}
