//! Per-connection state: the PTY registry and the sink that pushes terminal
//! output back over the socket.

use std::sync::Arc;

use phantom_core::fs::watch::FsWatchState;
use phantom_core::pty::{AgentSignal, OpenPty, PtySink, PtyState};
use phantom_core::pty::stream::{OutputQueue, PresentationClass};
use phantom_core::workspace::{WorkspaceEnv, WorkspaceRegistry};
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::protocol::{encode_pty_data, ServerMsg};

/// What the socket writer task accepts. Terminal output is pre-framed bytes;
/// everything else is JSON.
pub enum Outbound {
    Binary(Vec<u8>),
    Text(String),
    Stream(Vec<u8>, tokio::sync::OwnedSemaphorePermit),
    Close,
}

/// Bridges `phantom-core`'s PTY threads to the socket writer.
///
/// Stream clients publish into a bounded output queue. Legacy channel clients
/// retain their existing raw-frame delivery until the terminal migration.
pub struct SocketSink {
    pty_id: u32,
    tx: mpsc::UnboundedSender<Outbound>,
    output: Option<Arc<OutputQueue>>,
}

impl SocketSink {
    pub fn new(pty_id: u32, tx: mpsc::UnboundedSender<Outbound>, output: Option<Arc<OutputQueue>>) -> Self {
        Self { pty_id, tx, output }
    }

    fn send_json(&self, msg: &ServerMsg) {
        match serde_json::to_string(msg) {
            Ok(text) => {
                // A closed channel means the client disconnected; the PTY is
                // torn down separately. Not worth logging per message.
                let _ = self.tx.send(Outbound::Text(text));
            }
            Err(e) => log::error!("serializing ServerMsg failed: {e}"),
        }
    }
}

impl PtySink for SocketSink {
    fn output_queue(&self) -> Option<Arc<OutputQueue>> {
        self.output.clone()
    }

    fn data(&self, bytes: Vec<u8>) {
        let _ = self
            .tx
            .send(Outbound::Binary(encode_pty_data(self.pty_id, &bytes)));
    }

    fn exit(&self, code: i32) {
        self.send_json(&ServerMsg::PtyExit { id: self.pty_id, code });
    }

    fn agent_event(&self, signal: AgentSignal) {
        self.send_json(&ServerMsg::AgentSignal {
            id: signal.id,
            kind: signal.kind.to_string(),
            agent: signal.agent,
        });
    }
}

/// Arguments accepted by the `pty_open` command. Field names match what the
/// existing Terax frontend already sends, so `pty-bridge.ts` needs no change.
#[derive(Debug, Deserialize)]
pub struct PtyOpenArgs {
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    pub blocks: Option<bool>,
    #[serde(default)]
    pub shell: Option<String>,
    /// The leaf's stable session name for create-or-reuse (reload reattaches).
    /// Absent → a fresh throwaway session, as before.
    #[serde(default)]
    pub session: Option<String>,
    pub presentation: Option<PresentationClass>,
}

#[derive(Debug, Deserialize)]
pub struct PtyResizeArgs {
    pub id: u32,
    pub cols: u16,
    pub rows: u16,
}

#[derive(Debug, Deserialize)]
pub struct PtyIdArgs {
    pub id: u32,
}

/// Orchestration args. `cwd` locates `.phantom/agents.yaml`; `name` is the
/// agent key from that file.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentListArgs {
    pub cwd: String,
    #[serde(default)]
    pub workspace: Option<WorkspaceEnv>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentOpenArgs {
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub cwd: Option<String>,
}

/// Ensure a named agent session exists without wiring a terminal. The panel
/// calls this, then `pty_attach`es to view it — so the shell renders through a
/// real terminal rather than streaming into a sink nobody shows.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEnsureArgs {
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub cwd: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDispatchArgs {
    pub name: String,
    pub task: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNudgeArgs {
    pub name: String,
    pub msg: String,
    /// How long to wait for the agent to go idle before sending anyway.
    #[serde(default = "default_nudge_timeout")]
    pub timeout_ms: u64,
}

fn default_nudge_timeout() -> u64 {
    30_000
}

/// Arguments for `pty_attach`. Unlike `pty_open` there is no cwd, workspace, or
/// shell: the session already exists and keeps what it was created with.
#[derive(Debug, Deserialize)]
pub struct PtyAttachArgs {
    pub name: String,
    pub cols: u16,
    pub rows: u16,
    pub presentation: Option<PresentationClass>,
}

/// Arguments for `pty_kill` — end a session by name.
#[derive(Debug, Deserialize)]
pub struct PtyKillArgs {
    pub name: String,
}

/// Arguments for `orchestrate` — kick the brain off toward a goal.
///
/// The brain can be supplied inline from the Models-panel provider the user
/// already configured (so they don't set it twice): `endpoint` + `model`, with
/// the key read server-side from the secret named by `keyService`/`keyAccount`
/// (never sent over the wire). When omitted, it falls back to the
/// `orchestrator:` block in `.phantom/agents.yaml` + the `orchestrator/apiKey`
/// secret.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrateArgs {
    pub goal: String,
    pub cwd: String,
    #[serde(default)]
    pub workspace: Option<WorkspaceEnv>,
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub key_service: Option<String>,
    #[serde(default)]
    pub key_account: Option<String>,
}

/// State scoped to one WebSocket connection.
///
/// What a dropped socket does to the terminals depends on the backend: the
/// local one kills them, rmux only detaches, so they can be listed with
/// `pty_sessions` and picked back up with `pty_attach` from any device.
pub struct Connection {
    pub ai_stream: phantom_core::net::stream::AiStreamState,
    pub pty_stream: crate::pty_stream::PtyStream,
    pub pty: Arc<PtyState>,
    pub rmux: Arc<crate::rmux_pty::RmuxState>,
    pub registry: Arc<WorkspaceRegistry>,
    pub secrets: Arc<phantom_core::secrets::SecretsStore>,
    pub fs_watch: FsWatchState,
    /// Generation counter that lets a newer interactive search cancel the one
    /// still running. Per-connection, not global: two browsers typing different
    /// queries must not cancel each other. The desktop app keeps one app-wide
    /// because it has a single window, where the two are the same thing.
    pub content_search: phantom_core::fs::grep::ContentSearchState,
    /// Agent shell sessions and background processes, per-connection like the
    /// desktop's single app-wide state.
    pub shell: phantom_core::shell::ShellState,
    pub history: Arc<phantom_core::history::HistoryState>,
    pub rooms: Arc<crate::room::Rooms>,
    /// The server itself, for room delivery: it runs past the command that
    /// posted, and needs the workspace config to fetch remote references.
    pub app: Arc<crate::AppState>,
    fs_sink: FsSink,
    /// Set by `orchestrate_stop` to break a running orchestration loop. One per
    /// connection: a connection runs at most one orchestration at a time.
    orchestrator_cancel: Arc<std::sync::atomic::AtomicBool>,
    pub tx: mpsc::UnboundedSender<Outbound>,
}

/// Filesystem events reach the frontend through the same event names Terax's
/// desktop build emits, so the `listen()` call sites need no change.
struct FsSink {
    tx: mpsc::UnboundedSender<Outbound>,
    index: phantom_core::fs::index::FileIndexState,
}

impl FsSink {
    fn emit(&self, event: &str, payload: serde_json::Value) {
        let msg = serde_json::json!({ "type": "event", "event": event, "payload": payload });
        match serde_json::to_string(&msg) {
            Ok(text) => {
                let _ = self.tx.send(Outbound::Text(text));
            }
            Err(e) => log::error!("serializing {event} failed: {e}"),
        }
    }
}

impl phantom_core::fs::FsEventSink for FsSink {
    fn changed(&self, paths: Vec<String>) {
        for path in &paths {
            self.index.note_change(path);
        }
        self.emit("fs:changed", serde_json::json!({ "paths": paths }));
    }
}

impl phantom_core::fs::FileEventSink for FsSink {
    fn file_written(&self, path: String, source: Option<String>) {
        self.emit(
            "fs:file-written",
            serde_json::json!({ "path": path, "source": source }),
        );
    }
}

impl AsRef<dyn phantom_core::fs::FileEventSink> for Connection {
    fn as_ref(&self) -> &(dyn phantom_core::fs::FileEventSink + 'static) {
        &self.fs_sink
    }
}

impl Connection {
    /// Takes the whole [`AppState`] rather than five handles: everything a
    /// connection borrows from the server is already there, and the room made
    /// that list long enough that threading each one through was noise.
    pub fn new(state: &Arc<crate::AppState>, tx: mpsc::UnboundedSender<Outbound>) -> Self {
        Self {
            pty: Arc::new(PtyState::default()),
            ai_stream: Default::default(),
            pty_stream: crate::pty_stream::PtyStream::new(state.pty_stream.clone()),
            rmux: Arc::new(crate::rmux_pty::RmuxState::new(state.agents.clone(), state.rooms.clone())),
            registry: state.registry.clone(),
            secrets: state.secrets.clone(),
            rooms: state.rooms.clone(),
            app: state.clone(),
            fs_watch: FsWatchState::default(),
            content_search: Default::default(),
            shell: Default::default(),
            history: state.history.clone(),
            fs_sink: FsSink {
                tx: tx.clone(),
                index: state.file_index.clone(),
            },
            orchestrator_cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            tx,
        }
    }

    pub fn clone_as_fs_sink(&self) -> Arc<dyn phantom_core::fs::FsEventSink> {
        Arc::new(FsSink {
            tx: self.tx.clone(),
            index: self.app.file_index.clone(),
        })
    }

    fn pty_sink(&self, id: u32, class: Option<PresentationClass>) -> Result<Arc<dyn PtySink>, String> {
        let output = self.pty_stream.register(id, class.unwrap_or_default())?;
        Ok(Arc::new(SocketSink::new(id, self.tx.clone(), output)))
    }

    pub async fn dispatch_pty_stream(&self, cmd: &str, args: serde_json::Value) -> Option<Result<serde_json::Value, String>> {
        Some(match cmd {
            "pty_stream_open" => self.pty_stream.open().await.map(serde_json::Value::String),
            "pty_set_presentation" => {
                #[derive(Deserialize)]
                struct Report { ids: Vec<u32>, class: PresentationClass, seq: u64 }
                parse::<Report>(args).and_then(|a| self.pty_stream.set_presentation(a.ids, a.class, a.seq)).map(|()| serde_json::Value::Null)
            }
            _ => return None,
        })
    }

    /// PTY commands under the rmux backend. Separate from [`Self::dispatch`]
    /// because the SDK is async and the rest of the command surface is not;
    /// returns `None` for anything it doesn't own so the caller falls through.
    pub async fn dispatch_rmux_pty(
        &self,
        cmd: &str,
        args: serde_json::Value,
    ) -> Option<Result<serde_json::Value, String>> {
        Some(match cmd {
            "pty_open" => match parse::<PtyOpenArgs>(args) {
                Ok(a) => {
                    let id = self.pty_stream.next_id();
                    let sink = match self.pty_sink(id, a.presentation) {
                        Ok(sink) => sink,
                        Err(error) => return Some(Err(error)),
                    };
                    self.rmux
                        .open(
                            id,
                            &self.registry,
                            OpenPty {
                                cols: a.cols,
                                rows: a.rows,
                                cwd: a.cwd,
                                workspace: a.workspace,
                                blocks: a.blocks.unwrap_or(false),
                                shell: a.shell,
                            },
                            a.session,
                            sink,
                        )
                        .await
                        .inspect_err(|_| self.pty_stream.close(id))
                        .map(|()| serde_json::json!(id))
                }
                Err(e) => Err(e),
            },
            "pty_resize" => match parse::<PtyResizeArgs>(args) {
                Ok(a) => self
                    .rmux
                    .resize(a.id, a.cols, a.rows)
                    .await
                    .map(|()| serde_json::Value::Null),
                Err(e) => Err(e),
            },
            "pty_get_cwd" => match parse::<PtyIdArgs>(args) {
                Ok(a) => self.rmux.cwd(a.id).await.map(|cwd| serde_json::json!(cwd)),
                Err(e) => Err(e),
            },
            // Detach, don't kill: outliving the client is the whole point of
            // running terminals in rmux.
            "pty_close" => match parse::<PtyIdArgs>(args) {
                Ok(a) => {
                    self.pty_stream.close(a.id);
                    self.rmux.close(a.id).await.map(|()| serde_json::Value::Null)
                },
                Err(e) => Err(e),
            },
            "pty_close_all" => {
                self.pty_stream.close_all();
                Ok(serde_json::json!(self.rmux.detach_all().await))
            },
            // Explicit end-by-name, for a detached session (e.g. a reattach-banner
            // chip) that no local id points at. `pty_close` only detaches.
            "pty_kill" => match parse::<PtyKillArgs>(args) {
                Ok(a) => self
                    .rmux
                    .kill_session(&a.name)
                    .await
                    .map(|()| serde_json::Value::Null),
                Err(e) => Err(e),
            },
            // rmux-only, so the frontend must treat "unknown command" as "this
            // backend has no reattachable sessions" rather than an error.
            "pty_sessions" => self.rmux.list().await.map(|s| serde_json::json!(s)),
            "pty_attach" => match parse::<PtyAttachArgs>(args) {
                Ok(a) => {
                    let id = self.pty_stream.next_id();
                    let sink = match self.pty_sink(id, a.presentation) {
                        Ok(sink) => sink,
                        Err(error) => return Some(Err(error)),
                    };
                    self.rmux
                        .attach(id, &a.name, a.cols, a.rows, sink)
                        .await
                        .inspect_err(|_| self.pty_stream.close(id))
                        .map(|()| serde_json::json!(id))
                }
                Err(e) => Err(e),
            },
            // Orchestration (step 1): list the workspace's configured agents,
            // marking which have a live session, and open a named agent session.
            // rmux-only — the whole design rests on named sessions surviving a
            // reconnect, which the local backend cannot do.
            "agent_list" => match parse::<AgentListArgs>(args) {
                Ok(a) => self.agent_list(a.cwd, a.workspace).await,
                Err(e) => Err(e),
            },
            "agent_open" => match parse::<AgentOpenArgs>(args) {
                Ok(a) => {
                    let id = self.pty_stream.next_id();
                    let sink = match self.pty_sink(id, None) {
                        Ok(sink) => sink,
                        Err(error) => return Some(Err(error)),
                    };
                    self.rmux
                        .open_agent(id, &self.registry, crate::rmux_pty::OpenAgent {
                            name: a.name, cols: a.cols, rows: a.rows, cwd: a.cwd,
                        }, sink)
                        .await
                        .inspect_err(|_| self.pty_stream.close(id))
                        .map(|session| serde_json::json!({ "id": id, "session": session }))
                }
                Err(e) => Err(e),
            },
            "agent_ensure" => match parse::<AgentEnsureArgs>(args) {
                Ok(a) => self
                    .rmux
                    .ensure_agent_session(&a.name, &self.registry, a.cols, a.rows, a.cwd)
                    .await
                    .map(|session| serde_json::json!({ "session": session })),
                Err(e) => Err(e),
            },
            "agent_dispatch" => match parse::<AgentDispatchArgs>(args) {
                Ok(a) => self
                    .rmux
                    .send_to_agent(&a.name, &a.task)
                    .await
                    .map(|_| serde_json::json!({ "ok": true })),
                Err(e) => Err(e),
            },
            "agent_nudge" => match parse::<AgentNudgeArgs>(args) {
                Ok(a) => self
                    .rmux
                    .nudge(&a.name, &a.msg, a.timeout_ms)
                    .await
                    .map(|outcome| serde_json::json!({ "sent": outcome })),
                Err(e) => Err(e),
            },
            _ => return None,
        })
    }

    /// Reads `.phantom/agents.yaml` from the (authorized) workspace and marks
    /// each agent whose `phantom-agent-<name>` session is currently live.
    pub(crate) async fn agent_list(
        &self,
        cwd: String,
        workspace: Option<WorkspaceEnv>,
    ) -> Result<serde_json::Value, String> {
        let root = phantom_core::workspace::authorize(&cwd, workspace, &self.registry)?;
        let mut agents = crate::agents::load(std::path::Path::new(&root))?;
        let live: std::collections::HashSet<String> = self
            .rmux
            .list()
            .await?
            .into_iter()
            .map(|s| s.name)
            .collect();
        for a in &mut agents {
            a.running = live.contains(&format!("phantom-agent-{}", a.name));
        }
        Ok(serde_json::json!(agents))
    }

    /// Orchestrator (step 5): an external brain drives the same dispatch/nudge
    /// primitives. Async + streaming (each step goes out as `Stream { req }`),
    /// so it's routed here rather than through the blocking `dispatch`.
    pub async fn dispatch_orchestrator(
        &self,
        cmd: &str,
        args: serde_json::Value,
        req: u64,
    ) -> Option<Result<serde_json::Value, String>> {
        use std::sync::atomic::Ordering;
        Some(match cmd {
            "orchestrate" => match parse::<OrchestrateArgs>(args) {
                Ok(a) => self.run_orchestration(a, req).await,
                Err(e) => Err(e),
            },
            // Flips the shared flag the loop checks between steps. Runs in its
            // own spawned task (see ws.rs), so it lands while the loop awaits.
            "orchestrate_stop" => {
                self.orchestrator_cancel.store(true, Ordering::SeqCst);
                Ok(serde_json::Value::Null)
            }
            _ => return None,
        })
    }

    /// Streams one orchestration step to the panel, keyed by the request id.
    fn orch_emit(&self, req: u64, kind: &str, mut detail: serde_json::Value) {
        if let Some(obj) = detail.as_object_mut() {
            obj.insert("kind".into(), serde_json::json!(kind));
        }
        let msg = crate::protocol::ServerMsg::Stream {
            req,
            event: detail,
        };
        if let Ok(text) = serde_json::to_string(&msg) {
            let _ = self.tx.send(Outbound::Text(text));
        }
    }

    /// The loop: ask the brain, run whatever tools it calls against the real
    /// sessions, feed results back, until it calls `finish`, the step cap, or a
    /// stop. Content between agents is files; this only names sessions + pokes.
    async fn run_orchestration(
        &self,
        args: OrchestrateArgs,
        req: u64,
    ) -> Result<serde_json::Value, String> {
        use std::sync::atomic::Ordering;
        let OrchestrateArgs {
            goal,
            cwd,
            workspace,
            endpoint,
            model,
            key_service,
            key_account,
        } = args;
        let root = phantom_core::workspace::authorize(&cwd, workspace, &self.registry)?;
        let root_path = std::path::Path::new(&root);

        // Prefer the brain the frontend passed from the Models panel (set once),
        // reading its key from our own secrets by the given service/account so it
        // never crosses the wire. Fall back to the `.phantom/agents.yaml` block
        // + the `orchestrator/apiKey` secret.
        let (config, key) = match (endpoint, model) {
            (Some(endpoint), Some(model)) => {
                let key = match (&key_service, &key_account) {
                    (Some(s), Some(a)) => self.secrets.get(s, a)?,
                    _ => None,
                }
                .or_else(|| std::env::var("PHANTOM_ORCHESTRATOR_KEY").ok())
                .ok_or("orchestrator provider has no API key set")?;
                (crate::orchestrator::OrchestratorConfig { endpoint, model }, key)
            }
            _ => {
                let config = crate::orchestrator::load_config(root_path)?
                    .ok_or("no orchestrator provider chosen and no `orchestrator:` block in .phantom/agents.yaml")?;
                let key = match self.secrets.get("orchestrator", "apiKey")? {
                    Some(k) => k,
                    None => std::env::var("PHANTOM_ORCHESTRATOR_KEY").map_err(|_| {
                        "no orchestrator API key (secret orchestrator/apiKey)".to_string()
                    })?,
                };
                (config, key)
            }
        };

        let roster = self.roster_text(root_path).await?;
        let tools = crate::orchestrator::tools();
        let mut messages = vec![
            serde_json::json!({"role": "system", "content": crate::orchestrator::system_prompt(&roster)}),
            serde_json::json!({"role": "user", "content": goal}),
        ];
        let client = reqwest::Client::new();
        self.orchestrator_cancel.store(false, Ordering::SeqCst);
        const MAX_STEPS: usize = 16;

        for step in 0..MAX_STEPS {
            if self.orchestrator_cancel.load(Ordering::SeqCst) {
                self.orch_emit(req, "stopped", serde_json::json!({}));
                return Ok(serde_json::json!({"status": "stopped", "steps": step}));
            }
            let a = crate::orchestrator::call(&client, &config, &key, &messages, &tools).await?;
            messages.push(a.message.clone());

            if a.tool_calls.is_empty() {
                // The brain wrote prose instead of acting — surface it and stop.
                self.orch_emit(req, "note", serde_json::json!({"text": a.content}));
                return Ok(serde_json::json!({"status": "done", "reason": "no_tool_call"}));
            }
            for call in a.tool_calls {
                if call.name == "finish" {
                    let summary = call
                        .arguments
                        .get("summary")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.orch_emit(req, "finish", serde_json::json!({"summary": summary}));
                    return Ok(serde_json::json!({"status": "finished", "summary": summary}));
                }
                let result = self.exec_orch_tool(&call, root_path, req).await;
                let content = match &result {
                    Ok(s) => s.clone(),
                    Err(e) => format!("error: {e}"),
                };
                messages.push(serde_json::json!({
                    "role": "tool",
                    "tool_call_id": call.id,
                    "content": content,
                }));
            }
        }
        self.orch_emit(req, "note", serde_json::json!({"text": "reached step limit"}));
        Ok(serde_json::json!({"status": "max_steps"}))
    }

    /// Agent roster as prompt text: `- name (role) [running|idle]`.
    async fn roster_text(&self, root: &std::path::Path) -> Result<String, String> {
        let mut agents = crate::agents::load(root)?;
        let live: std::collections::HashSet<String> =
            self.rmux.list().await?.into_iter().map(|s| s.name).collect();
        for a in &mut agents {
            a.running = live.contains(&format!("phantom-agent-{}", a.name));
        }
        Ok(agents
            .iter()
            .map(|a| {
                format!(
                    "- {} ({}) [{}]",
                    a.name,
                    a.role,
                    if a.running { "running" } else { "idle" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// Runs one brain-requested tool against the real sessions, emits the step,
    /// and returns the string fed back to the brain as the tool result.
    async fn exec_orch_tool(
        &self,
        call: &crate::orchestrator::ToolCall,
        root: &std::path::Path,
        req: u64,
    ) -> Result<String, String> {
        let arg = |k: &str| call.arguments.get(k).and_then(|v| v.as_str()).unwrap_or("");
        match call.name.as_str() {
            "dispatch_agent" => {
                let (agent, task) = (arg("agent"), arg("task"));
                self.orch_emit(req, "dispatch", serde_json::json!({"agent": agent, "task": task}));
                self.rmux.send_to_agent(agent, task).await?;
                Ok(format!("dispatched to {agent}"))
            }
            "nudge_agent" => {
                let (agent, message) = (arg("agent"), arg("message"));
                self.orch_emit(req, "nudge", serde_json::json!({"agent": agent, "message": message}));
                let sent = self.rmux.nudge(agent, message, 30_000).await?;
                Ok(format!("nudged {agent} (sent when {sent})"))
            }
            "read_handoff" => {
                let file = arg("file");
                self.orch_emit(req, "read", serde_json::json!({"file": file}));
                let content = read_handoff_file(root, file)?;
                Ok(content)
            }
            "list_agents" => {
                self.orch_emit(req, "list", serde_json::json!({}));
                self.roster_text(root).await
            }
            other => Err(format!("unknown tool {other}")),
        }
    }

    /// Secret storage for the AI panel's API keys.
    ///
    /// Small enough to live here rather than in its own module: four arms, all
    /// single-word arguments, no camelCase conversion to get wrong.
    fn dispatch_secrets(
        &self,
        cmd: &str,
        args: serde_json::Value,
    ) -> Option<Result<serde_json::Value, String>> {
        #[derive(Deserialize)]
        struct Account {
            service: String,
            account: String,
        }
        #[derive(Deserialize)]
        struct SetArgs {
            service: String,
            account: String,
            password: String,
        }
        #[derive(Deserialize)]
        struct AllArgs {
            service: String,
            accounts: Vec<String>,
        }

        Some(match cmd {
            "secrets_get" => parse::<Account>(args)
                .and_then(|a| self.secrets.get(&a.service, &a.account))
                .map(|v| serde_json::json!(v)),
            "secrets_set" => parse::<SetArgs>(args)
                .and_then(|a| self.secrets.set(&a.service, &a.account, a.password))
                .map(|()| serde_json::Value::Null),
            "secrets_delete" => parse::<Account>(args)
                .and_then(|a| self.secrets.delete(&a.service, &a.account))
                .map(|()| serde_json::Value::Null),
            "secrets_get_all" => parse::<AllArgs>(args)
                .and_then(|a| self.secrets.get_all(&a.service, &a.accounts))
                .map(|v| serde_json::json!(v)),
            _ => return None,
        })
    }

    /// Dispatch one `invoke`. Returns the JSON result, or an error string that
    /// the caller turns into a `Result{err}` reply.
    ///
    /// Blocking: `pty_open` spawns a process. Callers on the async runtime must
    /// run this via `spawn_blocking`.
    pub fn dispatch(&self, cmd: &str, args: serde_json::Value) -> Result<serde_json::Value, String> {
        if cmd.starts_with("ai_session_") {
            return crate::ai_session_commands::dispatch(cmd, args)
                .unwrap_or_else(|| Err(format!("unknown command: {cmd}")));
        }
        // Filesystem commands live in their own module; it returns `None` when
        // the command isn't one of its own so we fall through to the rest.
        if let Some(result) = crate::fs_commands::dispatch(self, cmd, args.clone()) {
            return result;
        }
        // Same arrangement for git, which is a large surface of its own.
        if let Some(result) = crate::git_commands::dispatch(self, cmd, args.clone()) {
            return result;
        }
        if let Some(result) = self.dispatch_secrets(cmd, args.clone()) {
            return result;
        }
        if let Some(result) = crate::shell_commands::dispatch(self, cmd, args.clone()) {
            return result;
        }
        if let Some(result) = crate::history_commands::dispatch(self, cmd, args.clone()) {
            return result;
        }
        if let Some(result) = crate::agent_commands::dispatch(cmd, args.clone()) {
            return result;
        }
        match cmd {
            "pty_open" => {
                let a: PtyOpenArgs = parse(args)?;
                let id = self.pty_stream.next_id();
                let sink = self.pty_sink(id, a.presentation)?;
                self.pty.open(
                    id,
                    &self.registry,
                    OpenPty {
                        cols: a.cols,
                        rows: a.rows,
                        cwd: a.cwd,
                        workspace: a.workspace,
                        blocks: a.blocks.unwrap_or(false),
                        shell: a.shell,
                    },
                    sink,
                ).inspect_err(|_| self.pty_stream.close(id))?;
                Ok(serde_json::json!(id))
            }
            "pty_resize" => {
                let a: PtyResizeArgs = parse(args)?;
                self.pty.resize(a.id, a.cols, a.rows)?;
                Ok(serde_json::Value::Null)
            }
            "pty_close" => {
                let a: PtyIdArgs = parse(args)?;
                self.pty_stream.close(a.id);
                self.pty.close(a.id)?;
                Ok(serde_json::Value::Null)
            }
            "pty_close_all" => {
                self.pty_stream.close_all();
                Ok(serde_json::json!(self.pty.close_all()))
            },
            "pty_has_foreground_process" => {
                let a: PtyIdArgs = parse(args)?;
                Ok(serde_json::json!(self.pty.has_foreground_process(a.id)?))
            }
            "pty_has_foreground_job" => {
                let a: PtyIdArgs = parse(args)?;
                Ok(serde_json::json!(self.pty.has_foreground_job(a.id)?))
            }
            // Workspace resolution — the app shell asks for these while booting.
            "workspace_current_dir" => Ok(serde_json::json!(
                phantom_core::workspace::current_dir(&self.registry)?
            )),
            "workspace_authorize" => {
                #[derive(Deserialize)]
                struct Args {
                    path: String,
                    #[serde(default)]
                    workspace: Option<WorkspaceEnv>,
                }
                let a: Args = parse(args)?;
                Ok(serde_json::json!(phantom_core::workspace::authorize(
                    &a.path,
                    a.workspace,
                    &self.registry
                )?))
            }
            "pty_shell_name" => Ok(serde_json::json!(phantom_core::pty::shell_name())),
            "pty_list_shells" => {
                serde_json::to_value(phantom_core::pty::list_shells()).map_err(|e| e.to_string())
            }
            // Launch context: a browser tab is never opened "with a file", and
            // the working directory is the server's, not a CLI argument's.
            "get_launch_dir" => Ok(serde_json::Value::Null),
            "get_launch_files" => Ok(serde_json::json!([])),

            // Tauri plugins are stubbed out on the web (see the transport
            // shims). Answer null instead of erroring: these calls are all
            // optional capability probes, and failing them fills the log with
            // noise that hides real errors.
            other if other.starts_with("plugin:") => {
                log::debug!("ignoring call to stubbed plugin command: {other}");
                Ok(serde_json::Value::Null)
            }

            other => Err(format!("unknown command: {other}")),
        }
    }
}

fn parse<T: serde::de::DeserializeOwned>(args: serde_json::Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}

/// Reads a file under `<root>/.phantom/handoff/`, refusing anything that
/// escapes that directory. The brain names files it was told agents write, but
/// the name is still untrusted text, so a `..` or absolute path must not reach
/// outside the handoff dir. Content is capped so a huge file can't blow up the
/// prompt.
pub(crate) fn read_handoff_file(root: &std::path::Path, file: &str) -> Result<String, String> {
    const MAX: usize = 64 * 1024;
    let base = root.join(".phantom/handoff");
    let target = base.join(file);
    let canon_base = base
        .canonicalize()
        .map_err(|e| format!("no handoff dir: {e}"))?;
    let canon = target
        .canonicalize()
        .map_err(|e| format!("no such handoff file {file:?}: {e}"))?;
    if !canon.starts_with(&canon_base) {
        return Err(format!("{file:?} is outside the handoff directory"));
    }
    let mut content =
        std::fs::read_to_string(&canon).map_err(|e| format!("read {file:?}: {e}"))?;
    if content.len() > MAX {
        // On a char boundary: a byte cap landing inside a multi-byte character
        // panics, and on the peer link that panic ends the link for good.
        content.truncate(content.floor_char_boundary(MAX));
        content.push_str("\n…(truncated)");
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::read_handoff_file;

    #[test]
    fn truncating_a_large_handoff_respects_char_boundaries() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join(".phantom/handoff");
        std::fs::create_dir_all(&dir).unwrap();
        // 3-byte characters, so the 64 KiB cap falls mid-character.
        std::fs::write(dir.join("notes.md"), "中".repeat(30_000)).unwrap();

        let content = read_handoff_file(root.path(), "notes.md").unwrap();
        assert!(content.ends_with("…(truncated)"));
        assert!(content.len() <= 64 * 1024 + "\n…(truncated)".len());
    }
}
