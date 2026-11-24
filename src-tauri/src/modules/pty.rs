//! Tauri PTY commands; bounded output and scheduling live in phantom-core.

mod gate;
pub mod protocol;
pub mod stream;
pub use stream::NativePtyStream;

use std::sync::Arc;

// `lib.rs` registers `PtyState` by this module's path, so the import is public.
use phantom_core::pty::stream::{OutputQueue, PresentationClass};
pub use phantom_core::pty::PtyState;
use phantom_core::pty::{AgentSignal, OpenPty, PtySink, ShellInfo};
use phantom_core::workspace::{WorkspaceEnv, WorkspaceRegistry};
use tauri::{AppHandle, Emitter, Manager};

const AGENT_EVENT: &str = "terax:agent-signal";

struct StreamSink {
    app: AppHandle,
    output: Arc<OutputQueue>,
}

impl PtySink for StreamSink {
    fn output_queue(&self) -> Option<Arc<OutputQueue>> {
        Some(self.output.clone())
    }
    fn data(&self, _bytes: Vec<u8>) {}
    fn exit(&self, _code: i32) {}

    fn agent_event(&self, signal: AgentSignal) {
        let _ = self.app.emit(AGENT_EVENT, signal);
    }

    fn reap(&self, id: u32) {
        if let Some(state) = self.app.try_state::<PtyState>() {
            state.take(id);
        }
        if let Some(streams) = self.app.try_state::<NativePtyStream>() {
            streams.close(id);
        }
    }
}

/// Synchronous on purpose: spawning a shell blocks, and Tauri runs non-async
/// commands on its own thread pool rather than the async runtime — so this
/// needs no `spawn_blocking` and can borrow `State` directly.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn pty_open(
    app: AppHandle,
    webview: tauri::Webview,
    streams: tauri::State<'_, NativePtyStream>,
    state: tauri::State<'_, PtyState>,
    registry: tauri::State<'_, WorkspaceRegistry>,
    cols: u16,
    rows: u16,
    cwd: Option<String>,
    workspace: Option<WorkspaceEnv>,
    blocks: Option<bool>,
    shell: Option<String>,
    pane_id: Option<u32>,
    presentation: Option<PresentationClass>,
) -> Result<u32, String> {
    let _opening = streams.opening.lock().unwrap();
    let id = state.next_id();
    let _ = pane_id;
    let output = streams.register(
        id,
        webview.label(),
        presentation.unwrap_or(PresentationClass::Focused),
    );
    let sink = Arc::new(StreamSink { app, output });
    let opts = OpenPty {
        cols,
        rows,
        cwd,
        workspace,
        blocks: blocks.unwrap_or(false),
        shell,
    };
    let result = state.open(id, &registry, opts, sink);
    if result.is_err() {
        streams.close(id);
    }
    result
}

// Input is the latency-critical path: raw body + id header skips JSON
// serialization of every keystroke on both sides of the IPC boundary.
#[tauri::command]
pub fn pty_write(
    webview: tauri::Webview,
    streams: tauri::State<NativePtyStream>,
    state: tauri::State<PtyState>,
    request: tauri::ipc::Request,
) -> Result<(), String> {
    let id: u32 = request
        .headers()
        .get("x-pty-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| "pty_write: missing x-pty-id header".to_string())?;
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("pty_write: expected raw body".to_string());
    };
    if !streams.owns(id, webview.label()) {
        return Err("terminal belongs to another webview".into());
    }
    state.write(id, bytes)
}

#[tauri::command]
pub fn pty_resize(
    webview: tauri::Webview,
    streams: tauri::State<NativePtyStream>,
    state: tauri::State<PtyState>,
    id: u32,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    if !streams.owns(id, webview.label()) {
        return Err("terminal belongs to another webview".into());
    }
    state.resize(id, cols, rows)
}

#[tauri::command]
pub fn pty_close(
    webview: tauri::Webview,
    streams: tauri::State<NativePtyStream>,
    state: tauri::State<PtyState>,
    id: u32,
) -> Result<(), String> {
    if !streams.owns(id, webview.label()) {
        return Ok(());
    }
    streams.close(id);
    state.close(id)
}

#[tauri::command]
pub fn pty_close_all(
    webview: tauri::Webview,
    streams: tauri::State<NativePtyStream>,
    state: tauri::State<PtyState>,
) -> Result<usize, String> {
    Ok(streams.close_webview(webview.label(), &state))
}

#[tauri::command]
pub fn pty_has_foreground_process(state: tauri::State<PtyState>, id: u32) -> Result<bool, String> {
    state.has_foreground_process(id)
}

#[tauri::command]
pub fn pty_has_foreground_job(state: tauri::State<PtyState>, id: u32) -> Result<bool, String> {
    state.has_foreground_job(id)
}

#[tauri::command]
pub fn pty_shell_name() -> String {
    phantom_core::pty::shell_name()
}

#[tauri::command]
pub fn pty_list_shells() -> Vec<ShellInfo> {
    phantom_core::pty::list_shells()
}
