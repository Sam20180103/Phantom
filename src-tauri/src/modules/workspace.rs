//! Tauri command surface for workspace authorization and WSL discovery.
//!
//! This was a thousand-line copy of what is now `phantom_core::workspace` — the
//! extraction left the original behind, and the two drifted into a trap: the
//! app managed *this* file's `WorkspaceRegistry` while `pty` and `fs` asked for
//! core's, two distinct types wearing one name. Every command taking the
//! registry would have failed to resolve its state at runtime.
//!
//! So the logic lives in core and this file is the adapter, like every other
//! module here. Nothing is reimplemented; the commands only add `State`
//! extraction.

pub use phantom_core::workspace::{
    bootstrap_registry, init_launch_cwd, WorkspaceEnv, WorkspaceRegistry, WslDistro,
};

use phantom_core::workspace as core;

/// Synchronous, like the other blocking commands here: Tauri runs non-async
/// commands on its own thread pool, so a WSL subprocess cannot stall the async
/// runtime and needs no `spawn_blocking` of its own.
#[tauri::command]
pub fn workspace_authorize(
    path: String,
    workspace: Option<WorkspaceEnv>,
    registry: tauri::State<'_, WorkspaceRegistry>,
) -> Result<String, String> {
    core::authorize(&path, workspace, &registry)
}

#[tauri::command]
pub fn workspace_current_dir(
    registry: tauri::State<'_, WorkspaceRegistry>,
) -> Result<String, String> {
    core::current_dir(&registry)
}

#[tauri::command]
pub fn wsl_list_distros() -> Result<Vec<WslDistro>, String> {
    core::wsl_list_distros()
}

#[tauri::command]
pub fn wsl_default_distro() -> Result<Option<String>, String> {
    core::wsl_default_distro()
}

#[tauri::command]
pub fn wsl_home(distro: String) -> Result<String, String> {
    core::wsl_home(distro)
}
