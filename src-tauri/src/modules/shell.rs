//! Tauri command wrappers over `phantom_core::shell`. State extraction is the
//! only thing that differs from the headless server; the logic — timeouts,
//! output caps, background processes — lives in core so both share it.

// `lib.rs` registers `ShellState` by this module's path, so the import is public.
pub use phantom_core::shell::ShellState;
use phantom_core::shell::{
    self, background::BackgroundLogResponse, background::BackgroundProcInfo, session::SessionRunOutput,
    CommandOutput,
};
use phantom_core::workspace::{WorkspaceEnv, WorkspaceRegistry};

#[tauri::command]
pub fn shell_run_command(
    command: String,
    cwd: Option<String>,
    timeout_secs: Option<u64>,
    workspace: Option<WorkspaceEnv>,
    registry: tauri::State<'_, WorkspaceRegistry>,
) -> Result<CommandOutput, String> {
    shell::shell_run_command(command, cwd, timeout_secs, workspace, &registry)
}

#[tauri::command]
pub fn shell_session_open(
    state: tauri::State<ShellState>,
    registry: tauri::State<WorkspaceRegistry>,
    cwd: Option<String>,
    workspace: Option<WorkspaceEnv>,
) -> Result<u32, String> {
    shell::shell_session_open(&state, &registry, cwd, workspace)
}

#[tauri::command]
pub fn shell_session_run(
    state: tauri::State<'_, ShellState>,
    registry: tauri::State<'_, WorkspaceRegistry>,
    id: u32,
    command: String,
    cwd: Option<String>,
    timeout_secs: Option<u64>,
    workspace: Option<WorkspaceEnv>,
) -> Result<SessionRunOutput, String> {
    shell::shell_session_run(&state, &registry, id, command, cwd, timeout_secs, workspace)
}

#[tauri::command]
pub fn shell_session_close(state: tauri::State<ShellState>, id: u32) -> Result<(), String> {
    shell::shell_session_close(&state, id)
}

#[tauri::command]
pub fn shell_bg_spawn(
    state: tauri::State<ShellState>,
    registry: tauri::State<WorkspaceRegistry>,
    command: String,
    cwd: Option<String>,
    workspace: Option<WorkspaceEnv>,
) -> Result<u32, String> {
    shell::shell_bg_spawn(&state, &registry, command, cwd, workspace)
}

#[tauri::command]
pub fn shell_bg_logs(
    state: tauri::State<ShellState>,
    handle: u32,
    since_offset: Option<u64>,
) -> Result<BackgroundLogResponse, String> {
    shell::shell_bg_logs(&state, handle, since_offset)
}

#[tauri::command]
pub fn shell_bg_kill(state: tauri::State<ShellState>, handle: u32) -> Result<(), String> {
    shell::shell_bg_kill(&state, handle)
}

#[tauri::command]
pub fn shell_bg_list(
    state: tauri::State<ShellState>,
) -> Result<Vec<BackgroundProcInfo>, String> {
    shell::shell_bg_list(&state)
}
