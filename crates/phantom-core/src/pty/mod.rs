mod agent_detect;
mod da_filter;
mod session;
pub mod shell_init;
pub mod stream;

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;

use portable_pty::PtySize;

use crate::workspace::{user_spawn_cwd_or_home, WorkspaceEnv, WorkspaceRegistry};
use session::Session;

// `AgentDetector` is public because the rmux backend lives in `phantom-server`
// and has to run the same detection over the bytes rmux hands it — the local
// session below is no longer the only place terminal output flows through.
pub use agent_detect::{AgentDetector, AgentSignal, Transition};
pub use shell_init::ShellInfo;

/// Where a session's output and lifecycle events go.
///
/// This is the seam that replaced Terax's direct `tauri::ipc::Channel` and
/// `AppHandle::emit` calls. The desktop app implements it over Tauri channels;
/// the headless server implements it over a WebSocket. Nothing below this
/// trait knows which one is in use.
///
/// Credit-based clients return an output queue, which the reader uses directly.
/// Legacy clients receive non-blocking data/exit notifications instead.
pub trait PtySink: Send + Sync + 'static {
    fn output_queue(&self) -> Option<Arc<stream::OutputQueue>> {
        None
    }

    /// A chunk of terminal output. Already DA-filtered and coalesced.
    fn data(&self, bytes: Vec<u8>);

    /// The child exited. Always the last call for a session.
    fn exit(&self, code: i32);

    /// An AI agent changed state (parsed from OSC 777 markers).
    fn agent_event(&self, _signal: AgentSignal) {}

    /// The session ended and should be dropped from whatever registry owns it.
    /// Called from the waiter thread after [`Self::exit`].
    fn reap(&self, _id: u32) {}
}

pub struct PtyState {
    sessions: RwLock<HashMap<u32, Arc<Session>>>,
    // Starts at 1 so freshly-handed-out ids are never 0, which the frontend
    // sometimes treats as "unset". Increments monotonically; never reused.
    next_id: AtomicU32,
}

impl Default for PtyState {
    fn default() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
        }
    }
}

/// Everything needed to start a session, so callers don't juggle eight
/// positional arguments.
pub struct OpenPty {
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
    pub workspace: Option<WorkspaceEnv>,
    pub blocks: bool,
    pub shell: Option<String>,
}

impl PtyState {
    pub fn take(&self, id: u32) -> Option<Arc<Session>> {
        self.sessions.write().unwrap().remove(&id)
    }

    pub fn next_id(&self) -> u32 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Spawn a shell and register it. Blocking — the caller decides whether to
    /// run it on a blocking-friendly thread.
    pub fn open(
        &self,
        id: u32,
        registry: &WorkspaceRegistry,
        opts: OpenPty,
        sink: Arc<dyn PtySink>,
    ) -> Result<u32, String> {
        let workspace = WorkspaceEnv::from_option(opts.workspace);
        let cwd = user_spawn_cwd_or_home(registry, opts.cwd.as_deref(), &workspace);
        let session = session::spawn(
            id,
            opts.cols,
            opts.rows,
            cwd,
            workspace,
            opts.blocks,
            opts.shell,
            sink,
        )
        .map(|(s, _)| s)
        .map_err(|e| {
            log::error!("pty open failed: {e}");
            e
        })?;
        self.sessions.write().unwrap().insert(id, session);
        // The shell can exit before this insert (instant failure, `exit` in an
        // rc file); the waiter's reap then ran with the id absent. Re-check and
        // reap so the pseudoconsole isn't stranded.
        let exited = self
            .sessions
            .read()
            .unwrap()
            .get(&id)
            .map(|s| s.exited.load(Ordering::Acquire))
            .unwrap_or(false);
        if exited {
            if let Some(s) = self.take(id) {
                spawn_drop(id, s);
            }
        }
        log::info!("pty opened id={id} cols={} rows={}", opts.cols, opts.rows);
        Ok(id)
    }

    pub fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        let session = self.get(id)?;
        // Bind to a local so the MutexGuard temporary drops before `session` —
        // see rustc note on tail-expression temporary drop order.
        let result = session.writer.lock().unwrap().write_all(bytes).map_err(|e| {
            // EPIPE is expected if the child already exited.
            log::debug!("pty write id={id} failed: {e}");
            e.to_string()
        });
        result
    }

    pub fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.get(id)?;
        let result = session
            .master
            .lock()
            .unwrap()
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| {
                log::warn!("pty resize id={id} failed: {e}");
                e.to_string()
            });
        result
    }

    pub fn close(&self, id: u32) -> Result<(), String> {
        let session = self.sessions.write().unwrap().remove(&id);
        if let Some(s) = session {
            if let Err(e) = s.killer.lock().unwrap().kill() {
                // Non-fatal: the child may already have exited on its own (e.g.
                // the user ran `exit`). Log so this isn't invisible in debugging.
                log::debug!("pty close: kill id={id} returned {e}");
            }
            log::info!("pty closed id={id}");
            spawn_drop(id, s);
        } else {
            log::debug!("pty close: unknown id={id}");
        }
        Ok(())
    }

    /// A fresh client connection orphans the previous one's sessions in this
    /// still-running process; reap them before any new session spawns.
    pub fn close_all(&self) -> usize {
        let drained: Vec<(u32, Arc<Session>)> = {
            let mut sessions = self.sessions.write().unwrap();
            sessions.drain().collect()
        };
        let count = drained.len();
        for (id, s) in drained {
            if let Err(e) = s.killer.lock().unwrap().kill() {
                log::debug!("pty close_all: kill id={id} returned {e}");
            }
            spawn_drop(id, s);
        }
        if count > 0 {
            log::info!("pty close_all: reaped {count} orphaned session(s)");
        }
        count
    }

    pub fn has_foreground_process(&self, id: u32) -> Result<bool, String> {
        let shell_pid = self.get(id)?.shell_pid;
        if shell_pid == 0 {
            return Ok(false);
        }
        Ok(shell_has_children(shell_pid))
    }

    /// Foreground-only check for the renderer hibernation path: true while a
    /// job owns the tty (tcgetpgrp != shell pgid). Stricter and cheaper than
    /// [`Self::has_foreground_process`], which counts background children too.
    pub fn has_foreground_job(&self, id: u32) -> Result<bool, String> {
        let session = self.get(id)?;
        if session.shell_pid == 0 {
            return Ok(false);
        }
        #[cfg(unix)]
        {
            let leader = session.master.lock().unwrap().process_group_leader();
            Ok(matches!(leader, Some(pid) if pid > 0 && pid as u32 != session.shell_pid))
        }
        #[cfg(windows)]
        {
            Ok(shell_has_children(session.shell_pid))
        }
    }

    fn get(&self, id: u32) -> Result<Arc<Session>, String> {
        self.sessions
            .read()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| {
                log::warn!("pty: unknown id={id}");
                "no session".to_string()
            })
    }
}

// Detached: on Windows `ClosePseudoConsole` can block until conhost drains,
// which would freeze the caller's thread and stall the whole IPC/WS loop.
fn spawn_drop(id: u32, s: Arc<Session>) {
    thread::Builder::new()
        .name(format!("phantom-pty-drop-{id}"))
        .spawn(move || {
            let t0 = std::time::Instant::now();
            session::drop_session(s);
            log::info!(
                "pty session id={id} dropped in {}ms",
                t0.elapsed().as_millis()
            );
        })
        .expect("spawn pty drop thread");
}

pub fn shell_name() -> String {
    shell_init::detect_shell_name()
}

pub fn list_shells() -> Vec<ShellInfo> {
    shell_init::list_shells()
}

// pgrep -P exits 0 when shell_pid has at least one child, 1 when none.
#[cfg(unix)]
fn shell_has_children(shell_pid: u32) -> bool {
    std::process::Command::new("pgrep")
        .args(["-P", &shell_pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    /// Records everything the PTY layer pushes out, so a test can assert on the
    /// same seam the Tauri app and the WebSocket server plug into.
    #[derive(Default)]
    struct RecordingSink {
        data: Mutex<Vec<u8>>,
        exit_code: Mutex<Option<i32>>,
        reaped: Mutex<Vec<u32>>,
    }

    impl PtySink for RecordingSink {
        fn data(&self, bytes: Vec<u8>) {
            self.data.lock().unwrap().extend_from_slice(&bytes);
        }
        fn exit(&self, code: i32) {
            *self.exit_code.lock().unwrap() = Some(code);
        }
        fn reap(&self, id: u32) {
            self.reaped.lock().unwrap().push(id);
        }
    }

    impl RecordingSink {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.data.lock().unwrap()).into_owned()
        }
    }

    fn wait_until(deadline: Duration, mut done: impl FnMut() -> bool) -> bool {
        let until = Instant::now() + deadline;
        while Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn open(state: &PtyState, sink: Arc<RecordingSink>) -> u32 {
        let registry = WorkspaceRegistry::default();
        let id = state.next_id();
        state
            .open(
                id,
                &registry,
                OpenPty {
                    cols: 80,
                    rows: 24,
                    cwd: None,
                    workspace: None,
                    blocks: false,
                    // Bypass the user's configured shell so the test doesn't
                    // depend on whatever rc files happen to be on the host.
                    shell: Some("/bin/sh".to_string()),
                },
                sink,
            )
            .expect("open pty");
        id
    }

    // The end-to-end path this crate exists to make possible: spawn a shell,
    // write to it, and get output back through `PtySink` rather than through a
    // Tauri channel.
    #[test]
    fn round_trips_input_to_output_through_the_sink() {
        let state = PtyState::default();
        let sink = Arc::new(RecordingSink::default());
        let id = open(&state, sink.clone());

        state.write(id, b"echo phantom-ok\n").expect("write");

        assert!(
            wait_until(Duration::from_secs(10), || sink.text().contains("phantom-ok")),
            "expected shell output to reach the sink, got: {:?}",
            sink.text()
        );
        state.close(id).expect("close");
    }

    #[test]
    fn reports_exit_and_reaps_when_the_shell_exits() {
        let state = PtyState::default();
        let sink = Arc::new(RecordingSink::default());
        let id = open(&state, sink.clone());

        state.write(id, b"exit 7\n").expect("write");

        assert!(
            wait_until(Duration::from_secs(10), || sink
                .exit_code
                .lock()
                .unwrap()
                .is_some()),
            "shell exit never reached the sink"
        );
        assert_eq!(*sink.exit_code.lock().unwrap(), Some(7));
        assert_eq!(*sink.reaped.lock().unwrap(), vec![id], "must ask to be reaped");
    }

    #[test]
    fn writing_to_an_unknown_session_errors_rather_than_panicking() {
        let state = PtyState::default();
        assert!(state.write(999, b"x").is_err());
        assert!(state.resize(999, 80, 24).is_err());
        // Closing an unknown id is a no-op: a client may close twice on teardown.
        assert!(state.close(999).is_ok());
    }

    #[test]
    fn close_all_reaps_every_live_session() {
        let state = PtyState::default();
        let a = open(&state, Arc::new(RecordingSink::default()));
        let b = open(&state, Arc::new(RecordingSink::default()));
        assert_ne!(a, b, "ids must be unique");
        assert_eq!(state.close_all(), 2);
        assert_eq!(state.close_all(), 0);
    }
}

#[cfg(windows)]
fn shell_has_children(shell_pid: u32) -> bool {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32First, Process32Next, PROCESSENTRY32,
        TH32CS_SNAPPROCESS,
    };
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return false;
        }
        let mut entry: PROCESSENTRY32 = zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32>() as u32;
        let mut found = false;
        if Process32First(snapshot, &mut entry) != 0 {
            loop {
                if entry.th32ParentProcessID == shell_pid {
                    found = true;
                    break;
                }
                if Process32Next(snapshot, &mut entry) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snapshot);
        found
    }
}
