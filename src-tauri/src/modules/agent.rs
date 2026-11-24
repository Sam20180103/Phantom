//! Tauri command wrappers over `phantom_core::agent`. The hook-installation
//! logic lives in core so the headless server shares it; the Windows-only
//! conout marker is re-exported for `lib.rs`.

use phantom_core::agent;

// Windows-only in core: the marker is written to the console buffer, which
// has no counterpart elsewhere.
#[cfg(windows)]
pub use phantom_core::agent::emit_conout_marker;

#[tauri::command]
pub fn agent_enable_hooks(agent: String) -> Result<(), String> {
    agent::agent_enable_hooks(agent)
}

#[tauri::command]
pub fn agent_hooks_status(agent: String) -> bool {
    agent::agent_hooks_status(agent)
}
