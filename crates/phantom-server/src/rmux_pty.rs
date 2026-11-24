//! Terminals backed by the rmux daemon instead of a PTY this process owns.
//!
//! The point is outliving `phantom-server`. With the local backend a terminal
//! dies when this process does, so restarting to deploy kills whatever an agent
//! was in the middle of. rmux holds the shell in its own daemon, so a restart
//! costs a reconnect rather than the work.
//!
//! Opening and reattaching are deliberately *different* operations. Sessions
//! used to be named `phantom-<id>` and created with `create_or_reuse`, where
//! `<id>` came from a per-connection counter that restarts at 1 — so the first
//! terminal in a new browser tab silently adopted whatever was left running in
//! `phantom-1`, which for this project means walking into another agent's
//! session. Names now carry a timestamp and are created with `create_only`, and
//! taking over an existing session is an explicit [`RmuxState::attach`] after
//! [`RmuxState::list`].
//!
//! Lives here rather than in `phantom-core` for two reasons: the SDK is async
//! and this crate already owns the runtime, and `phantom-core` must keep
//! building for the desktop app, which has no rmux daemon.

use std::collections::HashMap;
use std::sync::Arc;

use phantom_core::pty::{AgentDetector, OpenPty, PtySink};
use phantom_core::workspace::{user_spawn_cwd_or_home, WorkspaceEnv, WorkspaceRegistry};
use rmux_sdk::{
    EnsureSession, ForegroundSource, Pane, PaneOutputChunk, PaneOutputStart, Rmux, Session,
    SessionName, TerminalSizeSpec,
};
use tokio::sync::Mutex;

/// Prefix for every session Phantom owns, so `rmux ls` on the host makes it
/// obvious which sessions came from here and a stray one can be cleaned up.
const SESSION_PREFIX: &str = "phantom-";

/// Orchestration agents get a further prefix so their stable, named sessions
/// are distinguishable from ordinary throwaway terminals in a listing.
const AGENT_PREFIX: &str = "phantom-agent-";

pub struct OpenAgent {
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub cwd: Option<String>,
}

/// One reattachable session, as shown to the client.
#[derive(serde::Serialize)]
pub struct SessionSummary {
    pub name: String,
    /// True when this server already has a terminal streaming it. Attaching
    /// again is allowed — two devices watching one agent is a feature here —
    /// but the client should say so rather than imply the session is idle.
    pub attached: bool,
}

/// A session name no live or future session can collide with.
///
/// The timestamp is what makes this safe across a `phantom-server` restart: pty
/// ids restart at 1, so an id alone would name an existing session again. The
/// counter disambiguates terminals opened within the same second.
fn fresh_session_name() -> String {
    use std::sync::atomic::{AtomicU32, Ordering};
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("{SESSION_PREFIX}{secs}-{seq}")
}

/// The `cmd` an agent is declared with in `<cwd>/.phantom/agents.yaml`.
///
/// `None` when there is no config, no such agent, or no cwd — the session then
/// stays the bare shell it was before, which is the old behaviour.
fn agent_start_command(cwd: Option<&str>, agent: &str) -> Option<String> {
    let agents = crate::agents::load(std::path::Path::new(cwd?)).ok()?;
    agents
        .into_iter()
        .find(|a| a.name == agent)
        .map(|a| a.cmd)
        .filter(|cmd| !cmd.trim().is_empty())
}

pub struct RmuxTerminal {
    pane: Pane,
    session_name: String,
    /// Pumps `pane.output_stream()` into the sink; aborted on close.
    reader: tokio::task::JoinHandle<()>,
    output: Option<Arc<phantom_core::pty::stream::OutputQueue>>,
}

impl Drop for RmuxTerminal {
    fn drop(&mut self) {
        if let Some(output) = &self.output {
            output.close();
        }
        self.reader.abort();
    }
}

/// How often the supervisor looks for agent sessions it isn't watching yet.
///
/// Only a backstop: sessions this server creates are watched the moment they
/// exist. This catches the ones it didn't — started before a restart, or by
/// another process — and re-covers any whose stream died.
const AGENT_WATCH_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// Tracks what every `phantom-agent-<name>` session is doing, by reading its
/// output for the OSC 777 markers the agent's hooks emit.
///
/// Server-wide and **viewer-independent**, which is the whole point. The state
/// is a property of the rmux session, not of anyone's terminal tab: `nudge`
/// consults it before injecting, and the room, an HTTP post and a peer link all
/// nudge with no browser attached anywhere. When the only reader was the one
/// `attach` spawns for a viewer, an unattended agent read as permanently idle
/// and got written into mid-task — exactly what waiting for idle is meant to
/// prevent.
#[derive(Default)]
pub struct AgentTracker {
    /// Last marker seen per session. No entry = no hook seen yet (a bare shell,
    /// or a CLI not yet launched), which counts as idle.
    states: std::sync::Mutex<HashMap<String, &'static str>>,
    /// Sessions with a live watcher, so two never race on the same pane.
    watched: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl AgentTracker {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn state(&self, session: &str) -> Option<&'static str> {
        self.states.lock().expect("agent states poisoned").get(session).copied()
    }

    /// Drops everything known about a session that is going away.
    pub fn forget(&self, session: &str) {
        self.states.lock().expect("agent states poisoned").remove(session);
        self.watched.lock().expect("agent watch set poisoned").remove(session);
    }

    /// Starts watching `session` unless it already is.
    ///
    /// Called when this server creates an agent session, so a brand-new agent
    /// is covered immediately rather than at the supervisor's next sweep.
    pub fn ensure_watching(self: &Arc<Self>, session: String) {
        if !self.claim(&session) {
            return;
        }
        let tracker = self.clone();
        tokio::spawn(async move {
            if let Err(e) = tracker.watch(&session).await {
                log::debug!("agent watch for {session} ended: {e}");
            }
            // Leave `states` alone: the last marker is still the best guess
            // until a fresh watcher says otherwise. Only stop claiming to watch.
            tracker.watched.lock().expect("agent watch set poisoned").remove(&session);
        });
    }

    /// Takes responsibility for watching `session`, or reports that something
    /// else already has.
    ///
    /// Split from [`Self::ensure_watching`] so the guard can be tested without
    /// spawning anything: the watcher connects to the daemon, and
    /// `connect_or_start` would start one from a unit test.
    fn claim(&self, session: &str) -> bool {
        // A plain terminal session has no agent whose hooks could fire; a
        // stream per tab would be pure waste.
        if !session.starts_with(AGENT_PREFIX) {
            return false;
        }
        self.watched
            .lock()
            .expect("agent watch set poisoned")
            .insert(session.to_string())
    }

    /// Reads one session's output until it ends, recording every marker.
    ///
    /// Starts at `Oldest`, not `Now`: replaying the daemon's retained backlog
    /// leaves the state at the *last* marker the session ever emitted, which is
    /// its true current state. Starting live would miss the `working` of a task
    /// already in flight and call a busy agent idle.
    async fn watch(&self, session: &str) -> Result<(), String> {
        let rmux = connect_rmux().await?;
        let session_name = SessionName::try_from(session.to_string())
            .map_err(|e| format!("invalid session name {session:?}: {e}"))?;
        let handle = rmux
            .session(session_name)
            .await
            .map_err(|e| format!("no such session: {e}"))?;
        let mut stream = handle
            .pane(0, 0)
            .output_stream_starting_at(PaneOutputStart::Oldest)
            .await
            .map_err(|e| format!("opening the output stream failed: {e}"))?;

        log::debug!("watching agent session {session}");
        let mut detect = AgentDetector::new();
        loop {
            match stream.next().await {
                Ok(Some(PaneOutputChunk::Bytes { bytes, .. })) => {
                    detect.process(&bytes, |t| {
                        let kind = t.into_signal(0).kind;
                        self.states
                            .lock()
                            .expect("agent states poisoned")
                            .insert(session.to_string(), kind);
                    });
                }
                Ok(Some(_)) => {}
                // The stream ends when the session's shell exits.
                Ok(None) => break,
                Err(e) => return Err(e.to_string()),
            }
        }
        // The agent is gone, not idle; a stale marker would mislead `nudge`.
        self.states.lock().expect("agent states poisoned").remove(session);
        Ok(())
    }

    /// Sweeps for agent sessions with no watcher, forever.
    pub async fn supervise(self: Arc<Self>) {
        loop {
            match connect_rmux().await {
                Ok(rmux) => match rmux.list_sessions().await {
                    Ok(names) => {
                        for name in names {
                            self.ensure_watching(name.to_string());
                        }
                    }
                    Err(e) => log::debug!("listing sessions for the agent watcher: {e}"),
                },
                // The daemon may not be up yet, or at all under the local
                // backend. Keep sweeping rather than giving up for good.
                Err(e) => log::debug!("agent watcher cannot reach rmux: {e}"),
            }
            tokio::time::sleep(AGENT_WATCH_POLL).await;
        }
    }
}

/// Connects to the daemon, starting one if it isn't running.
async fn connect_rmux() -> Result<Rmux, String> {
    Rmux::builder()
        .connect_or_start()
        .await
        .map_err(|e| format!("connecting to the rmux daemon failed: {e}"))
}

/// Whether a shell is the pane's foreground process. `None` when the daemon
/// can't say — no reading, or one taken from the pane's root process, which is
/// always the shell and so says nothing about what runs in front of it.
async fn foreground_is_shell(pane: &Pane) -> Option<bool> {
    let fg = pane.foreground_state().await.ok()??;
    if fg.sources.command != Some(ForegroundSource::Process) {
        return None;
    }
    Some(is_shell(fg.command.as_deref()?))
}

/// Waits for a just-started CLI to take the pane's foreground, then for it to
/// settle. A new pane may have no foreground reading for its first probes; one
/// that never gets one leaves nothing to wait on but the settle time.
async fn wait_for_cli(pane: &Pane) -> Result<(), String> {
    let deadline = std::time::Instant::now() + CLI_START_TIMEOUT;
    let mut unread = 0;
    loop {
        match foreground_is_shell(pane).await {
            Some(false) => break,
            Some(true) => {}
            None if unread >= 10 => break,
            None => unread += 1,
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "its CLI did not start within {}s",
                CLI_START_TIMEOUT.as_secs()
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }
    tokio::time::sleep(CLI_SETTLE).await;
    Ok(())
}

/// Whether an agent's last hook marker means typing into it now would land in
/// the middle of something: a task under way, or — `attention` — a question
/// it is waiting for a person to answer, where the keystrokes would be taken
/// as the answer.
fn is_busy(state: Option<&str>) -> bool {
    matches!(state, Some("working" | "started" | "attention"))
}

fn room_submission_key(agent: &str, state: Option<&str>) -> Option<&'static str> {
    match state {
        Some("working" | "started") if agent == "codex" => Some("\t"),
        state if is_busy(state) => None,
        _ => Some("\r"),
    }
}

fn is_shell(command: &str) -> bool {
    // A login shell reports as `-bash`; a path or `.exe` names the same thing.
    let name = command.rsplit(['/', '\\']).next().unwrap_or(command);
    let name = name.trim_start_matches('-').to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    matches!(
        name,
        "sh" | "bash" | "zsh" | "dash" | "fish" | "ksh" | "mksh" | "yash" | "tcsh" | "csh"
            | "nu" | "xonsh" | "elvish" | "busybox" | "pwsh" | "powershell" | "cmd"
    )
}

pub struct RmuxState {
    terminals: Mutex<HashMap<u32, RmuxTerminal>>,
    /// Read by `nudge` to decide when a target agent is idle enough to inject
    /// into. Owned by the server, not by this connection.
    agents: Arc<AgentTracker>,
    /// Consulted when creating an agent session, to put its room credential in
    /// the environment.
    rooms: Arc<crate::room::Rooms>,
    /// One lock per agent being woken, held from the check through the CLI
    /// taking the foreground. Two messages arriving while an agent starts
    /// would otherwise both see a shell prompt and both type its command.
    waking: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

/// How long a just-started CLI gets to take the pane's foreground.
const CLI_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Grace after the CLI takes the foreground and before anything is typed into
/// it: a TUI that hasn't drawn its input yet can drop keys, or leave them in
/// the terminal for the shell to read once the CLI exits.
const CLI_SETTLE: std::time::Duration = std::time::Duration::from_secs(3);

impl RmuxState {
    /// Shares one server-wide [`AgentTracker`]; see its docs for why.
    pub fn new(agents: Arc<AgentTracker>, rooms: Arc<crate::room::Rooms>) -> Self {
        Self { terminals: Mutex::new(HashMap::new()), agents, rooms, waking: Default::default() }
    }

    /// Makes sure `agent`'s CLI is running, so a room message can reach it:
    /// creates its session if there is none, starts its CLI again if the
    /// session is back at a shell prompt, then waits for the CLI to take the
    /// foreground.
    ///
    /// Only agents declared in `<workspace>/.phantom/agents.yaml`. Anything
    /// else has no command to start, and a session without one is just a
    /// shell — exactly what delivery refuses to type into.
    pub async fn wake_agent(
        &self,
        agent: &str,
        workspace: &std::path::Path,
        registry: &WorkspaceRegistry,
    ) -> Result<(), String> {
        let cwd = workspace.to_string_lossy().into_owned();
        let Some(cmd) = agent_start_command(Some(&cwd), agent) else {
            // Relative: this ends up in a notice everyone in the room reads.
            return Err(format!("{agent:?} is not declared in .phantom/agents.yaml"));
        };
        let lock = self
            .waking
            .lock()
            .expect("waking poisoned")
            .entry(agent.to_string())
            .or_default()
            .clone();
        let _one_at_a_time = lock.lock().await;

        let name = format!("{AGENT_PREFIX}{agent}");
        let session_name = SessionName::try_from(name.clone())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;
        let rmux = connect_rmux().await?;
        let pane = match rmux.session(session_name).await {
            Ok(session) => {
                let pane = session.pane(0, 0);
                if !self.at_shell_prompt(&name, &pane).await {
                    return Ok(());
                }
                // The configured command, typed where the last one exited: the
                // same thing opening the agent does, never anything a message
                // said.
                pane.send_text(&format!("{cmd}\r"))
                    .await
                    .map_err(|e| format!("restarting {agent:?} failed: {e}"))?;
                pane
            }
            // A fresh session is a shell, and `ensure_agent` types the command.
            Err(_) => {
                let (_, session) = self.ensure_agent(agent, registry, 120, 40, Some(cwd)).await?;
                self.agents.ensure_watching(name.clone());
                session.pane(0, 0)
            }
        };
        log::info!("started agent {agent} for a room message");
        wait_for_cli(&pane).await
    }


    pub async fn open(
        &self,
        id: u32,
        registry: &WorkspaceRegistry,
        options: OpenPty,
        session: Option<String>,
        sink: Arc<dyn PtySink>,
    ) -> Result<(), String> {
        let OpenPty { cols, rows, cwd, workspace, blocks, shell } = options;
        let workspace = WorkspaceEnv::from_option(workspace);
        let resolved_cwd = user_spawn_cwd_or_home(registry, cwd.as_deref(), &workspace);
        // Terax's shell integration — the rc-file injection that makes agent
        // detection work — has to survive the handover to rmux, so we hand the
        // daemon the same argv and environment we would have spawned ourselves.
        let spec = phantom_core::pty::shell_init::build_launch_spec(
            resolved_cwd,
            workspace,
            blocks,
            shell,
        )?;

        let rmux = connect_rmux().await?;
        // A client-supplied name is the leaf's *stable* identity: reuse it so a
        // page reload reattaches the same shell instead of orphaning it. Without
        // one, fall back to a fresh throwaway name that must never be reused.
        // Either way the name is validated so a client can't reach a non-Phantom
        // session or masquerade as an agent one.
        let (name, reuse) = match session {
            Some(s) => {
                if !s.starts_with(SESSION_PREFIX) || s.starts_with(AGENT_PREFIX) {
                    return Err(format!("{s:?} is not a valid terminal session name"));
                }
                (s, true)
            }
            None => (fresh_session_name(), false),
        };
        let session_name = SessionName::try_from(name.clone())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;

        let ensure0 = EnsureSession::try_named(session_name)
            .map_err(|e| format!("naming session {name:?} failed: {e}"))?;
        // `create_or_reuse` only for a stable name unique to this leaf, so it
        // can't adopt another leaf's or device's shell. A throwaway name stays
        // `create_only`.
        let mut ensure = if reuse {
            ensure0.create_or_reuse()
        } else {
            ensure0.create_only()
        }
        .detached(true)
        .size(TerminalSizeSpec::new(cols, rows))
        .argv(spec.argv.clone())
        .environment(spec.env.clone());
        if let Some(dir) = spec.cwd.as_deref() {
            ensure = ensure.working_directory(dir);
        }

        let session = rmux
            .ensure_session(ensure)
            .await
            .map_err(|e| format!("creating rmux session {name:?} failed: {e}"))?;
        let pane = session.pane(0, 0);
        // A reused session may have backlog (the reason it exists — reload gets
        // its scrollback back), so replay from `Oldest`; a fresh one has none,
        // so `Oldest` is just its startup banner.
        let start = if reuse {
            PaneOutputStart::Oldest
        } else {
            PaneOutputStart::Now
        };
        self.wire(id, name, pane, start, sink).await?;
        log::info!("rmux terminal opened id={id} cols={cols} rows={rows} reuse={reuse}");
        Ok(())
    }

    /// Opens (or re-opens) a named orchestration agent, e.g. `claude`.
    ///
    /// Unlike [`Self::open`], the session name is *stable* — `phantom-agent-<name>`
    /// — and the policy is `create_or_reuse`: an agent has one identity, so
    /// re-opening it after a restart or from another device must pick the same
    /// running shell back up, not spawn a second one. The collision hazard that
    /// makes plain terminals use `create_only` does not apply, because the name
    /// is meaningful rather than a recycled counter.
    ///
    /// Returns the resolved session name so the caller can address it later.
    pub async fn open_agent(
        &self,
        id: u32,
        registry: &WorkspaceRegistry,
        options: OpenAgent,
        sink: Arc<dyn PtySink>,
    ) -> Result<String, String> {
        let OpenAgent { name: agent, cols, rows, cwd } = options;
        let (name, session) = self
            .ensure_agent(&agent, registry, cols, rows, cwd)
            .await?;
        let pane = session.pane(0, 0);
        // `Oldest`: re-opening an agent should show what it did before, the same
        // reason attach replays backlog.
        self.wire(id, name.clone(), pane, PaneOutputStart::Oldest, sink)
            .await?;
        log::info!("rmux agent session opened id={id} agent={agent}");
        Ok(name)
    }

    /// Create-or-reuse the `phantom-agent-<name>` session *without* wiring a
    /// local pty to it. The UI path: a panel starts an agent, then reaches it
    /// with the ordinary `attach` (which wires a real terminal renderer) — so
    /// no output stream is pumped into a sink no one is showing. Returns the
    /// resolved session name and the live session handle.
    async fn ensure_agent(
        &self,
        agent: &str,
        registry: &WorkspaceRegistry,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
    ) -> Result<(String, Session), String> {
        let workspace = WorkspaceEnv::Local;
        let resolved_cwd = user_spawn_cwd_or_home(registry, cwd.as_deref(), &workspace);
        let spec =
            phantom_core::pty::shell_init::build_launch_spec(resolved_cwd, workspace, false, None)?;

        let rmux = connect_rmux().await?;
        let name = format!("{AGENT_PREFIX}{agent}");
        let session_name = SessionName::try_from(name.clone())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;

        // The agent's room credential goes in its environment, not in the
        // command it is told to run: a token in argv lands in the shell's
        // history, in `ps`, and in the agent's own scrollback.
        let mut env = spec.env.clone();
        env.extend(self.rooms.agent_env(agent));

        let mut ensure = EnsureSession::try_named(session_name)
            .map_err(|e| format!("naming session {name:?} failed: {e}"))?
            .create_or_reuse()
            .detached(true)
            .size(TerminalSizeSpec::new(cols, rows))
            .argv(spec.argv.clone())
            .environment(env);
        if let Some(dir) = spec.cwd.as_deref() {
            ensure = ensure.working_directory(dir);
        }

        // Was it already there? `create_or_reuse` doesn't say, and the answer
        // decides whether the agent's CLI still has to be started: a reused
        // session already runs it, a fresh one is a bare shell.
        let existed = rmux
            .session(
                SessionName::try_from(name.clone())
                    .map_err(|e| format!("invalid session name {name:?}: {e}"))?,
            )
            .await
            .is_ok();

        let session = rmux
            .ensure_session(ensure)
            .await
            .map_err(|e| format!("opening agent session {name:?} failed: {e}"))?;

        // A new session is only a shell — nothing has run `claude`/`agy`/… yet,
        // which is why "Open" used to land on a plain prompt. Type the agent's
        // `cmd` from `.phantom/agents.yaml`, the same way the desktop launcher
        // starts an agent in its pane.
        if !existed {
            if let Some(cmd) = agent_start_command(spec.cwd.as_deref(), agent) {
                session
                    .pane(0, 0)
                    .send_text(&format!("{cmd}\r"))
                    .await
                    .map_err(|e| format!("starting agent {agent:?} failed: {e}"))?;
            }
        }
        Ok((name, session))
    }

    /// Ensure the agent's session exists (create-or-reuse), returning its name.
    /// The panel calls this, then `attach`es to view it.
    pub async fn ensure_agent_session(
        &self,
        agent: &str,
        registry: &WorkspaceRegistry,
        cols: u16,
        rows: u16,
        cwd: Option<String>,
    ) -> Result<String, String> {
        let (name, _session) = self.ensure_agent(agent, registry, cols, rows, cwd).await?;
        // No terminal is wired here — the panel creates the session first and
        // attaches separately, and an orchestrator may never attach at all. Pick
        // it up now rather than waiting for the supervisor's next sweep.
        self.agents.ensure_watching(name.clone());
        log::info!("ensured agent session {name}");
        Ok(name)
    }

    /// Takes over an already-running session under a new local pty id.
    ///
    /// The counterpart to [`Self::open`]: the shell is already there, so
    /// nothing is spawned and no launch spec is built — the session keeps the
    /// environment it was created with, which is the point of reattaching.
    pub async fn attach(
        &self,
        id: u32,
        name: &str,
        cols: u16,
        rows: u16,
        sink: Arc<dyn PtySink>,
    ) -> Result<(), String> {
        // Attaching is reachable by any authenticated client, so the name it
        // supplies is constrained to sessions Phantom created. Without this a
        // client could reach any session on the host, including ones started
        // outside Phantom.
        if !name.starts_with(SESSION_PREFIX) {
            return Err(format!("{name:?} is not a Phantom session"));
        }

        let rmux = connect_rmux().await?;
        let session_name = SessionName::try_from(name.to_string())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;

        let ensure = EnsureSession::try_named(session_name)
            .map_err(|e| format!("naming session {name:?} failed: {e}"))?
            // Fail rather than resurrect: if the session died while the client
            // was showing it in a list, the user needs to know that, not get a
            // fresh shell wearing the old name.
            .reuse_only()
            .detached(true)
            .size(TerminalSizeSpec::new(cols, rows));

        let session = rmux
            .ensure_session(ensure)
            .await
            .map_err(|e| format!("attaching to rmux session {name:?} failed: {e}"))?;
        let pane = session.pane(0, 0);
        // `Oldest`, not the default `Now`: reattaching to a blank screen loses
        // exactly what the user came back for — what the agent did while they
        // were away. The daemon's retained backlog is replayed first, then live
        // output continues, so xterm rebuilds both the screen and its
        // scrollback from the raw bytes.
        //
        // Bounded by the daemon's retention, so a long-running session shows
        // its tail rather than its whole life.
        self.wire(id, name.to_string(), pane, PaneOutputStart::Oldest, sink)
            .await?;
        log::info!("rmux terminal attached id={id} session={name}");
        Ok(())
    }

    /// Names of the Phantom sessions the daemon currently has, and whether this
    /// server is already streaming each one.
    pub async fn list(&self) -> Result<Vec<SessionSummary>, String> {
        let rmux = connect_rmux().await?;
        let names = rmux
            .list_sessions()
            .await
            .map_err(|e| format!("listing rmux sessions failed: {e}"))?;

        let attached: std::collections::HashSet<String> = self
            .terminals
            .lock()
            .await
            .values()
            .map(|t| t.session_name.clone())
            .collect();

        Ok(names
            .into_iter()
            .map(|n| n.to_string())
            .filter(|n| n.starts_with(SESSION_PREFIX))
            .map(|name| SessionSummary { attached: attached.contains(&name), name })
            .collect())
    }

    /// Streams a pane's output into `sink` and records the terminal.
    ///
    /// Shared by [`Self::open`] and [`Self::attach`] — from here on the two are
    /// the same thing, which is what makes a reattached terminal behave
    /// identically to a fresh one.
    async fn wire(
        &self,
        id: u32,
        name: String,
        pane: Pane,
        start: PaneOutputStart,
        sink: Arc<dyn PtySink>,
    ) -> Result<(), String> {
        let mut stream = pane
            .output_stream_starting_at(start)
            .await
            .map_err(|e| format!("opening the output stream for {name:?} failed: {e}"))?;

        // Agent detection has to happen here, not in `phantom-core`: with this
        // backend the bytes never pass through the local session reader that
        // normally runs the detector, so without this the OSC 777 markers are
        // simply dropped and the agent panel stays empty.
        //
        // Hook state is not recorded here. `AgentTracker` watches every agent
        // session on its own, so a viewer attaching or leaving changes nothing
        // about what `nudge` sees — which is the point, since most injections
        // happen with nobody watching.
        self.agents.ensure_watching(name.clone());

        let output = sink.output_queue();
        let reader_output = output.clone();
        let reader = tokio::spawn(async move {
            let mut agent_detect = AgentDetector::new();
            let mut queries = crate::rmux_queries::QueryFilter::default();
            loop {
                match stream.next().await {
                    Ok(Some(PaneOutputChunk::Bytes { bytes, .. })) => {
                        agent_detect.process(&bytes, |t| sink.agent_event(t.into_signal(id)));
                        let output = queries.process(&bytes);
                        if !output.is_empty() {
                            if let Some(queue) = &reader_output {
                                if !queue.push_async(&output).await {
                                    break;
                                }
                            } else {
                                sink.data(output);
                            }
                        }
                    }
                    // Other chunk kinds carry stream metadata, not terminal
                    // bytes; nothing to forward.
                    Ok(Some(_)) => {}
                    // The stream ends when the pane's shell exits.
                    Ok(None) => break,
                    Err(e) => {
                        log::warn!("rmux output stream for pty {id} ended: {e}");
                        break;
                    }
                }
            }
            let remaining = queries.finish();
            if !remaining.is_empty() {
                if let Some(queue) = &reader_output {
                    queue.push_async(&remaining).await;
                } else {
                    sink.data(remaining);
                }
            }
            agent_detect.finish(|t| sink.agent_event(t.into_signal(id)));
            if let Some(queue) = &reader_output {
                queue.finish_reading();
                queue.set_exit_code(0);
                queue.begin_exit(std::time::Duration::from_secs(10));
            } else {
                sink.exit(0);
            }
            sink.reap(id);
        });

        self.terminals.lock().await.insert(
            id,
            RmuxTerminal { pane, session_name: name, reader, output },
        );
        Ok(())
    }

    pub async fn write(&self, id: u32, bytes: &[u8]) -> Result<(), String> {
        // `send_text` sends bytes literally (`literal: true` on the wire), so
        // escape sequences and control characters pass through unchanged.
        // Fall back to lossy conversion if arbitrary binary input is passed.
        let text = match std::str::from_utf8(bytes) {
            Ok(s) => std::borrow::Cow::Borrowed(s),
            Err(_) => String::from_utf8_lossy(bytes),
        };
        let terminals = self.terminals.lock().await;
        let terminal = terminals.get(&id).ok_or_else(|| "no session".to_string())?;
        terminal
            .pane
            .send_text(text.as_ref())
            .await
            .map_err(|e| format!("rmux write to pty {id} failed: {e}"))
    }

    /// Sends a line of text to a named agent session, resolved by name rather
    /// than by a local pty id.
    ///
    /// This is the orchestration primitive: the caller (a manual button, or
    /// later Qwen/cloud-API) hands work to `phantom-agent-<agent>` without
    /// owning a terminal viewport on it. Fails if the agent session isn't
    /// running — dispatch never resurrects a dead agent, that's `agent_open`'s job.
    ///
    /// A multi-line task is wrapped in a bracketed paste (`ESC[200~ … ESC[201~`)
    /// so a TUI like claude receives it as one paste and submits only on the
    /// trailing `\r`, instead of submitting at the first embedded newline and
    /// running a half task. Single-line tasks are sent plainly to avoid relying
    /// on bracketed-paste mode being on.
    pub async fn send_to_agent(&self, agent: &str, task: &str) -> Result<(), String> {
        self.send_agent_input(agent, task, false).await
    }

    async fn send_agent_input(&self, agent: &str, task: &str, room: bool) -> Result<(), String> {
        let name = format!("{AGENT_PREFIX}{agent}");
        let session_name = SessionName::try_from(name.clone())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;
        let rmux = connect_rmux().await?;
        let session = rmux
            .session(session_name)
            .await
            .map_err(|_| format!("agent {agent:?} is not running"))?;
        let pane = session.pane(0, 0);
        // An agent session is a login shell with the CLI typed into it. Once
        // the CLI quits, that shell is what's left, and it would run the task
        // as a command line — and room messages, which anyone in the room can
        // write, arrive through here too.
        if self.at_shell_prompt(&name, &pane).await {
            return Err(format!(
                "agent {agent:?} is at a shell prompt, not running its CLI; start it again"
            ));
        }
        let submission_key = || {
            if room {
                room_submission_key(agent, self.agents.state(&name))
                    .ok_or_else(|| format!("agent {agent:?} became unavailable for input"))
            } else {
                Ok("\r")
            }
        };
        let key = submission_key()?;
        // Tab queues a follow-up in Codex while its current turn is running.
        if key == "\t" {
            return pane
                .send_text(format!("\x1b[200~{task}\x1b[201~\t"))
                .await
                .map_err(|e| format!("queueing a follow-up for agent {agent:?} failed: {e}"));
        }
        let payload = if task.contains('\n') || (room && agent == "codex") {
            format!("\x1b[200~{task}\x1b[201~")
        } else {
            task.to_string()
        };
        pane.send_text(&payload)
            .await
            .map_err(|e| format!("dispatch to agent {agent:?} failed: {e}"))?;
        // Codex groups rapid input as a paste, including an immediately following Enter.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if self.at_shell_prompt(&name, &pane).await {
            return Err(format!("agent {agent:?} exited before submission"));
        }
        pane.send_text(submission_key()?)
            .await
            .map_err(|e| format!("submitting to agent {agent:?} failed: {e}"))
    }

    /// Whether the agent's pane is sitting at its shell rather than its CLI.
    ///
    /// The daemon's foreground reading is the direct answer, but it is probed
    /// about once a second, while the hook tracker hears a CLI exit the moment
    /// shell integration reports it. When the tracker says "exited" and the
    /// reading still names the CLI, the reading may be stale — so look again
    /// after the probe has run. (The CLI may also have been restarted by hand,
    /// which the tracker only learns at its first hook; the second look is
    /// what tells the two apart.)
    async fn at_shell_prompt(&self, session: &str, pane: &Pane) -> bool {
        let exited = self.agents.state(session) == Some("exited");
        match foreground_is_shell(pane).await {
            Some(true) => true,
            Some(false) if exited => {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                foreground_is_shell(pane).await != Some(false)
            }
            Some(false) => false,
            // No reading to go on: trust the tracker.
            None => exited,
        }
    }

    /// Waits for a named agent to look idle, then dispatches `msg` to it.
    ///
    /// "Busy" is [`is_busy`]; anything else — including no hook seen at all (a
    /// bare shell, or a CLI that hasn't launched) — counts as idle and sends
    /// immediately. This is decision 3 of the orchestration design: injecting
    /// into a CLI mid-task corrupts the running command, so hold until it is
    /// back at a prompt. Gives up after `timeout_ms` and sends anyway, so a
    /// silent or hook-less agent can't hang the orchestrator. Returns the state
    /// it acted on (`"idle"` or `"timeout"`).
    pub async fn nudge(&self, agent: &str, msg: &str, timeout_ms: u64) -> Result<&'static str, String> {
        let idle = self
            .wait_for_agent(agent, std::time::Duration::from_millis(timeout_ms), |state| !is_busy(state))
            .await;
        self.send_to_agent(agent, msg).await?;
        Ok(if idle { "idle" } else { "timeout" })
    }

    /// Delivers a Room message when the CLI can receive it. Codex can queue
    /// follow-ups during a turn. Approval prompts still block delivery.
    pub async fn deliver_room_message(
        &self,
        agent: &str,
        msg: &str,
        timeout: std::time::Duration,
    ) -> Result<bool, String> {
        if !self
            .wait_for_agent(agent, timeout, |state| room_submission_key(agent, state).is_some())
            .await
        {
            return Ok(false);
        }
        self.send_agent_input(agent, msg, true).await.map(|()| true)
    }

    /// What `agent`'s terminal shows right now, as plain text — the room's
    /// read-only look at an agent at work. Trailing blank lines are dropped.
    pub async fn peek(&self, agent: &str) -> Result<String, String> {
        let name = format!("{AGENT_PREFIX}{agent}");
        let session_name = SessionName::try_from(name.clone())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;
        let rmux = connect_rmux().await?;
        let session = rmux
            .session(session_name)
            .await
            .map_err(|_| format!("agent {agent:?} is not running"))?;
        let screen = session
            .pane(0, 0)
            .capture_pane()
            .join_wrapped(true)
            .await
            .map_err(|e| format!("reading {agent:?}'s screen failed: {e}"))?;
        Ok(String::from_utf8_lossy(&screen.stdout).trim_end().to_string())
    }

    /// Waits, up to `max`, for `agent`'s hooks to report it has taken up what
    /// was just typed. Right after an injection it can still read as idle, and
    /// the next message queued for it must not mistake that for a free agent.
    /// A hook-less agent never reports, so it simply gets `max` between messages.
    pub async fn settle(&self, agent: &str, max: std::time::Duration) {
        let name = format!("{AGENT_PREFIX}{agent}");
        let deadline = std::time::Instant::now() + max;
        while std::time::Instant::now() < deadline && !is_busy(self.agents.state(&name)) {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// True once the agent's hook state allows input, false on timeout.
    ///
    /// ponytail: 200ms poll rather than event wait — a handoff fires rarely and
    /// polling can't lose a wakeup; make it event-driven only if it gets hot.
    async fn wait_for_agent(
        &self,
        agent: &str,
        timeout: std::time::Duration,
        ready: impl Fn(Option<&str>) -> bool,
    ) -> bool {
        let name = format!("{AGENT_PREFIX}{agent}");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if ready(self.agents.state(&name)) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    pub async fn cwd(&self, id: u32) -> Result<Option<String>, String> {
        let pane = self
            .terminals
            .lock()
            .await
            .get(&id)
            .ok_or("no session")?
            .pane
            .clone();
        let foreground = pane.foreground_state().await.map_err(|e| e.to_string())?;
        Ok(foreground
            .filter(|state| matches!(state.sources.cwd, Some(ForegroundSource::Process | ForegroundSource::RootProcess)))
            .and_then(|state| state.cwd)
            .filter(|cwd| std::path::Path::new(cwd).is_absolute()))
    }

    pub async fn resize(&self, id: u32, cols: u16, rows: u16) -> Result<(), String> {
        // Clone the handle + name, then drop the lock before any await.
        let (pane, session_name) = {
            let terminals = self.terminals.lock().await;
            let t = terminals.get(&id).ok_or_else(|| "no session".to_string())?;
            (t.pane.clone(), t.session_name.clone())
        };
        // Resize the WINDOW, not just the pane. Our sessions are detached with no
        // sized client, so the window stays at its creation size and `resize-pane`
        // is capped by it — the shell never actually grows (a terminal opened
        // small, e.g. an agent session, stayed 80x24). Resizing the window lifts
        // that cap so the pane fills it and SIGWINCH reaches the shell.
        if let Ok(name) = SessionName::try_from(session_name) {
            if let Ok(session) = connect_rmux().await?.session(name).await {
                let _ = session.window(0).resize(Some(cols), Some(rows)).await;
            }
        }
        pane.resize(TerminalSizeSpec::new(cols, rows))
            .await
            .map_err(|e| format!("rmux resize of pty {id} failed: {e}"))
    }

    /// Detaches from a terminal without killing it.
    ///
    /// Deliberately *not* a kill: the whole reason to use rmux is that a shell
    /// survives its client. Use [`Self::kill`] when the user really means to
    /// end the session.
    pub async fn close(&self, id: u32) -> Result<(), String> {
        if let Some(terminal) = self.terminals.lock().await.remove(&id) {
            terminal.reader.abort();
            log::info!(
                "detached from rmux session {} (still running)",
                terminal.session_name
            );
        }
        Ok(())
    }

    pub async fn kill(&self, id: u32) -> Result<(), String> {
        let Some(terminal) = self.terminals.lock().await.remove(&id) else {
            return Ok(());
        };
        terminal.reader.abort();
        let rmux = connect_rmux().await?;
        let name = SessionName::try_from(terminal.session_name.clone())
            .map_err(|e| format!("invalid session name: {e}"))?;
        match rmux.session(name).await {
            // `kill` reports whether a session was actually there; a `false`
            // just means it had already exited, which is not an error here.
            Ok(session) => session
                .kill()
                .await
                .map(|_| ())
                .map_err(|e| format!("killing rmux session failed: {e}")),
            Err(e) => {
                log::debug!("rmux session {} already gone: {e}", terminal.session_name);
                Ok(())
            }
        }
    }

    /// Kills a session by name — the path for ending a *detached* session that
    /// no local pty is wired to, e.g. a chip in the reattach banner. Closing a
    /// terminal only detaches (so it survives a reconnect); this is the explicit
    /// "I'm done with it" that actually ends the shell, sparing the user an SSH
    /// `rmux kill-session`.
    ///
    /// Name is prefix-checked for the same reason `attach` is: any authenticated
    /// client reaches this, and it must not be able to kill a session Phantom
    /// did not create. If a local terminal happens to be wired to the name, its
    /// reader is torn down too so no orphan pump keeps running.
    pub async fn kill_session(&self, name: &str) -> Result<(), String> {
        if !name.starts_with(SESSION_PREFIX) {
            return Err(format!("{name:?} is not a Phantom session"));
        }
        // Drop any local terminal wired to this name, and its reader.
        let wired: Vec<u32> = self
            .terminals
            .lock()
            .await
            .iter()
            .filter(|(_, t)| t.session_name == name)
            .map(|(id, _)| *id)
            .collect();
        for id in wired {
            if let Some(t) = self.terminals.lock().await.remove(&id) {
                t.reader.abort();
            }
        }
        self.agents.forget(name);

        let rmux = connect_rmux().await?;
        let session_name = SessionName::try_from(name.to_string())
            .map_err(|e| format!("invalid session name {name:?}: {e}"))?;
        match rmux.session(session_name).await {
            Ok(session) => session
                .kill()
                .await
                .map(|_| ())
                .map_err(|e| format!("killing rmux session {name:?} failed: {e}")),
            // Already gone is success — the goal state is "not running".
            Err(e) => {
                log::debug!("rmux session {name} already gone: {e}");
                Ok(())
            }
        }
    }

    /// Detach every terminal this connection was attached to. Called when the
    /// socket drops — the sessions keep running for the next connection.
    pub async fn detach_all(&self) -> usize {
        let drained: Vec<(u32, RmuxTerminal)> =
            self.terminals.lock().await.drain().collect();
        let count = drained.len();
        for (_, terminal) in drained {
            terminal.reader.abort();
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::{agent_start_command, is_shell, AgentTracker, AGENT_PREFIX};

    #[tokio::test]
    async fn only_a_declared_agent_is_ever_started() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phantom")).unwrap();
        std::fs::write(dir.path().join(".phantom/agents.yaml"), "agents:\n  claude:\n    cmd: claude\n")
            .unwrap();
        let rooms = std::sync::Arc::new(crate::room::Rooms::new(&crate::config::Room {
            name: "phantom-test-wake".into(),
            ..Default::default()
        }));
        let rmux = super::RmuxState::new(AgentTracker::new(), rooms);
        let registry = phantom_core::workspace::WorkspaceRegistry::default();
        // A typo, or a name from another machine: there is no command to run,
        // and a session without one would be a bare shell.
        let err = rmux.wake_agent("claudee", dir.path(), &registry).await.unwrap_err();
        assert!(err.contains("not declared"), "{err}");
    }

    #[test]
    fn an_agent_mid_task_or_mid_question_is_busy() {
        use super::is_busy;
        for state in ["working", "started", "attention"] {
            assert!(is_busy(Some(state)), "{state}");
        }
        // Finished, never hooked, or its CLI gone (the shell guard's business,
        // not the idle wait's): nothing to wait for.
        for state in [Some("finished"), Some("exited"), None] {
            assert!(!is_busy(state), "{state:?}");
        }
    }

    #[test]
    fn room_queues_codex_follow_ups_without_answering_approval_prompts() {
        use super::room_submission_key;

        for state in ["working", "started"] {
            assert_eq!(room_submission_key("codex", Some(state)), Some("\t"));
            assert_eq!(room_submission_key("agy", Some(state)), None);
        }
        assert_eq!(room_submission_key("codex", Some("attention")), None);
        assert_eq!(room_submission_key("codex", Some("finished")), Some("\r"));
    }

    #[test]
    fn a_shell_is_recognised_however_it_is_named() {
        // Typing into any of these runs the text as a command line.
        for name in ["bash", "-bash", "/usr/bin/zsh", "fish", "pwsh.exe", "C:\\Windows\\cmd.exe"] {
            assert!(is_shell(name), "{name} is a shell");
        }
        // An agent CLI, or whatever wraps it, is where a task belongs.
        for name in ["claude", "node", "codex", "gemini", "opencode", "cat", "bashtop"] {
            assert!(!is_shell(name), "{name} is not a shell");
        }
    }

    #[test]
    fn unknown_session_is_idle_and_forget_clears_state() {
        let tracker = AgentTracker::new();
        // No marker seen yet reads as idle, which is what lets a bare shell or
        // a not-yet-launched CLI be dispatched to at all.
        assert_eq!(tracker.state("phantom-agent-claude"), None);
        tracker
            .states
            .lock()
            .unwrap()
            .insert("phantom-agent-claude".to_string(), "working");
        assert_eq!(tracker.state("phantom-agent-claude"), Some("working"));
        tracker.forget("phantom-agent-claude");
        assert_eq!(tracker.state("phantom-agent-claude"), None);
    }

    #[test]
    fn only_agent_sessions_are_watched_and_never_twice() {
        let tracker = AgentTracker::new();
        // A plain terminal session has no agent whose hooks could fire.
        assert!(!tracker.claim("phantom-mtgo7ah4-1jz7b5vm"));

        let name = format!("{AGENT_PREFIX}claude");
        assert!(tracker.claim(&name));
        // Two watchers on one pane would double every marker and race on the
        // state, so the second caller must be told to stand down.
        assert!(!tracker.claim(&name));

        // After the session ends, the next sweep has to be able to pick it up.
        tracker.forget(&name);
        assert!(tracker.claim(&name));
    }

    #[test]
    fn start_command_comes_from_agents_yaml() {
        let dir = std::env::temp_dir().join(format!("phantom-startcmd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".phantom")).unwrap();
        std::fs::write(
            dir.join(".phantom/agents.yaml"),
            "agents:\n  claude:\n    cmd: claude\n",
        )
        .unwrap();
        let cwd = dir.to_str().unwrap();
        assert_eq!(agent_start_command(Some(cwd), "claude").as_deref(), Some("claude"));
        // Unknown agent, or no workspace: leave the session a bare shell.
        assert_eq!(agent_start_command(Some(cwd), "agy"), None);
        assert_eq!(agent_start_command(None, "claude"), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
