//! The cross-machine link: a peer server dials the hub and its agents join the
//! room.
//!
//! No new daemon. Every machine already runs a `phantom-server` for its own
//! browser, so the peer link is just that server holding one outbound WebSocket
//! to whichever server hosts the room. Everything a peer does with its local
//! agents — `nudge`, the handoff directory, the hook-idle wait — is the code
//! that was already there; this module only decides *which* machine acts.
//!
//! Roles:
//!
//! * **Hub** — owns the log, assigns `seq`, fans every message out to browsers
//!   and to all peers, and proxies reference fetches between them.
//! * **Peer** — forwards its own posts upstream, mirrors what comes back into
//!   its local log (so its own browser shows the same room), and injects into
//!   the agent sessions it owns.
//!
//! The one thing a message never carries is content. A [`Ref`] names a handoff
//! file; the receiving peer fetches it and writes it under `.phantom/room-cache/`
//! *before* injecting, so the agent is handed a real path on its own disk and
//! never has to understand any of this.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot};

use super::{deliver_to, Participant, Ref, RoomMsg, Rooms};
use crate::AppState;

/// Reconnect backoff bounds for the peer's dialer.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// How long to wait for the other side to answer a reference fetch.
const REF_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a queued message waits for its agent to be ready for input.
/// Long, because only that agent's queue waits; when it runs out the message
/// is not typed in — the room says so instead.
const DELIVER_WAIT: Duration = Duration::from_secs(10 * 60);

/// How long, after typing a message in, to wait for the agent's hooks to say
/// it took it up, before the next queued message starts waiting for idle.
const SETTLE: Duration = Duration::from_secs(5);

/// Cap on inbound posts per peer, per minute. A compromised machine on the mesh
/// must not be able to flood the log or wedge every agent with injections.
const PEER_POST_BUDGET: u32 = 120;

/// How often the hub pings each peer. Only the hub pings — both ends answer
/// pings on their own, so one side asking keeps traffic flowing both ways.
const PING_EVERY: Duration = Duration::from_secs(20);

/// Silence longer than this means the link is dead even if TCP hasn't noticed.
/// A laptop that slept or a mesh route that vanished leaves a half-open socket
/// that looks connected for many minutes: the peer keeps posting into it, and
/// the hub keeps offering that peer's agents for mentions nobody will receive.
const LINK_IDLE: Duration = Duration::from_secs(3 * PING_EVERY.as_secs());

/// The most of a screen sent in one look. A terminal is rarely more than a few
/// KiB of text; this is a bound, not a budget.
const MAX_SCREEN: usize = 32 * 1024;

/// How often a peer checks whether its roster changed. The hub only knows which
/// agents are running from what the peer tells it.
const ROSTER_EVERY: Duration = Duration::from_secs(10);

/// One frame of the peer protocol. Symmetric — both ends send and receive the
/// same enum, which keeps request/response pairs from drifting apart.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Frame {
    /// Peer → hub, first frame after connecting. `since` is the newest sequence
    /// the peer already mirrored, so the hub can replay what it missed while
    /// disconnected — a dropped mesh link must not punch a hole in its log.
    Hello {
        peer: String,
        agents: Vec<RemoteAgent>,
        #[serde(default)]
        since: Option<u64>,
        /// The log the peer's copy belongs to. `since` means nothing against
        /// a different one. Absent from older builds.
        #[serde(default)]
        epoch: Option<String>,
    },
    /// Hub → peer, answering [`Frame::Hello`]. The room's name belongs to the
    /// hub — it is one room — so a peer displays what it is told rather than
    /// whatever its own config happens to say.
    Welcome {
        room: String,
        #[serde(default)]
        epoch: Option<String>,
        /// The peer's copy is of another log: set it aside before the replay,
        /// which starts from the beginning of this one.
        #[serde(default)]
        reset: bool,
    },
    /// Peer → hub, when its roster changes.
    Roster { agents: Vec<RemoteAgent> },
    /// Hub → peer: every agent in the room, on joining and whenever that
    /// changes, so any machine's browser can offer any machine's agents.
    Directory { members: Vec<Member> },
    /// Peer → hub: something on that machine spoke. `agent: None` is its human.
    /// `notice` marks the peer's server reporting something — a delivery that
    /// failed there — rather than anyone on it speaking.
    Post {
        #[serde(default)]
        agent: Option<String>,
        text: String,
        #[serde(default)]
        refs: Vec<Ref>,
        #[serde(default)]
        notice: bool,
    },
    /// Hub → peer: an authoritative message, already sequenced. `replay` marks
    /// a backfilled one, which is mirrored but never injected — re-nudging an
    /// agent with week-old work is worse than the gap it repairs.
    Deliver {
        msg: RoomMsg,
        #[serde(default)]
        replay: bool,
    },
    RefRequest { req: u64, r: Ref },
    /// What an agent's screen shows, asked of the machine it runs on and
    /// answered with a [`Frame::RefResponse`] carrying the text.
    PeekRequest { req: u64, peer: String, agent: String },
    RefResponse {
        req: u64,
        #[serde(default)]
        ok: Option<String>,
        #[serde(default)]
        err: Option<String>,
    },
}

/// An agent on some other machine, as its own server reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RemoteAgent {
    pub name: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub running: bool,
    /// What its hooks last reported — `working`, `attention`, `finished`,
    /// `exited` — so whoever is about to hand it work can see whether it is
    /// free. Absent from older builds, and before the first hook fires.
    #[serde(default)]
    pub state: Option<String>,
}

/// One agent in the hub's directory: which machine it is on, and its state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Member {
    pub peer: String,
    #[serde(flatten)]
    pub agent: RemoteAgent,
}

// ---------------------------------------------------------------- hub side

/// `GET /room/ws` — a peer server joining.
///
/// Outside the browser's session middleware: a peer authenticates with the
/// long-lived token in its config, not with a login cookie. Failures answer 401
/// with no body, so probing tells an attacker nothing about which names exist.
pub async fn hub_ws(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
    let Some(peer) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .and_then(|t| state.config.room.peer_for_token(t.trim()))
        .map(str::to_string)
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if state.rooms.is_peer() {
        // A server that is itself a peer has no log to be the source of truth.
        log::warn!("refusing peer {peer:?}: this server is a peer, not a hub");
        return StatusCode::CONFLICT.into_response();
    }
    ws.on_upgrade(move |socket| serve_peer(socket, state, peer))
}

async fn serve_peer(socket: WebSocket, state: Arc<AppState>, peer: String) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();

    let writer = tokio::spawn(async move {
        let mut ping = tokio::time::interval(PING_EVERY);
        loop {
            let msg = tokio::select! {
                frame = rx.recv() => {
                    let Some(frame) = frame else { break };
                    let Ok(text) = serde_json::to_string(&frame) else { continue };
                    Message::Text(text.into())
                }
                _ = ping.tick() => Message::Ping(Default::default()),
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
    });

    log::info!("peer {peer:?} joined the room");
    state.rooms.attach_peer(&peer, tx.clone(), Vec::new());
    let mut budget = Budget::new(PEER_POST_BUDGET);

    loop {
        let text = match tokio::time::timeout(LINK_IDLE, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => break,
            // A pong answering our ping: arriving at all was its job.
            Ok(Some(Ok(_))) => continue,
            Err(_) => {
                log::warn!("peer {peer:?} went silent; dropping its link");
                break;
            }
        };
        let Ok(frame) = serde_json::from_str::<Frame>(&text) else {
            log::warn!("peer {peer:?} sent an unparseable frame");
            continue;
        };
        match frame {
            Frame::Hello { agents, since, epoch, .. } => {
                state.rooms.attach_peer(&peer, tx.clone(), agents);
                let ours = state.rooms.epoch();
                let (since, reset) = replay_plan(&ours, epoch.as_deref(), since);
                let room = state.rooms.name.clone();
                let _ = tx.send(Frame::Welcome { room, epoch: Some(ours), reset });
                for msg in state.rooms.history(since) {
                    let _ = tx.send(Frame::Deliver { msg, replay: true });
                }
            }
            Frame::Roster { agents } => {
                state.rooms.attach_peer(&peer, tx.clone(), agents);
            }
            Frame::Post { agent, text, refs, notice } => {
                if !budget.allow() {
                    log::warn!("peer {peer:?} exceeded its post budget; dropping");
                    continue;
                }
                let from = match agent {
                    Some(a) => Participant::agent(&peer, a),
                    None => Participant::human(&peer),
                };
                if notice {
                    // Same limits as a message: a notice is still text a peer
                    // wrote into everyone's log.
                    match super::validate(&text, &[]) {
                        Ok(text) => state.rooms.notice_from(Participant::human(&peer), text),
                        Err(e) => log::warn!("dropping a notice from peer {peer:?}: {e}"),
                    }
                    continue;
                }
                match state.rooms.post(from, text, refs) {
                    Ok(Some(msg)) => fanout(&state, &msg),
                    Ok(None) => {} // unreachable on a hub, which never forwards
                    Err(e) => log::warn!("dropping a post from peer {peer:?}: {e}"),
                }
            }
            Frame::RefRequest { req, r } => {
                // The hub is also the router: a file owned by a *third* machine
                // has to be fetched from that machine, not read off the hub's
                // own disk, which would silently answer with the wrong file.
                let state = state.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let answer = resolve_ref(&state, r).await;
                    let _ = tx.send(reply(req, answer));
                });
            }
            Frame::PeekRequest { req, peer: owner, agent } => {
                // Routed like a reference: served here if the agent is ours,
                // asked of the machine that runs it otherwise.
                let state = state.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(reply(req, peek(&state, &owner, &agent, false).await));
                });
            }
            Frame::RefResponse { req, ok, err } => state.rooms.settle_ref(req, ok, err),
            Frame::Deliver { .. } | Frame::Welcome { .. } | Frame::Directory { .. } => {
                log::warn!("peer {peer:?} sent a hub-only frame");
            }
        }
    }

    state.rooms.detach_peer(&peer, &tx);
    writer.abort();
    log::info!("peer {peer:?} left the room");
}

/// What a joining peer is replayed, and whether its copy must start over.
///
/// A copy of another log — the hub's was replaced, or the peer has none yet —
/// gets the whole window, since its `since` counts in someone else's numbers.
/// A copy of this one just catches up. An older build sends no epoch and keeps
/// the old behaviour.
fn replay_plan(ours: &str, theirs: Option<&str>, since: Option<u64>) -> (Option<u64>, bool) {
    match theirs {
        Some(theirs) if theirs != ours => (None, true),
        _ => (since, false),
    }
}

/// Starts delivering a hub-sequenced message to this server's own agents, and
/// reports mentions of machines that aren't connected.
///
/// Peers already have the message: [`Rooms::post`] relays every message to all
/// of them as it is numbered — every peer, not just the ones mentioned, since a
/// peer's own browser renders the same room from its mirror of the log.
///
/// Local delivery runs in the background. It may fetch references from other
/// machines and wait for an agent to accept input. Posting stays independent
/// of delivery so the browser, peer read loop, and agents can keep responding.
pub fn fanout(state: &Arc<AppState>, msg: &RoomMsg) {
    // A mention of a machine that isn't connected goes nowhere. Say so now,
    // rather than leave whoever asked waiting on an answer that can't come.
    for peer in state.rooms.absent_peers(msg) {
        state.rooms.notice(format!("{peer} is not in the room, so nobody there was told"));
    }
    deliver(state, msg);
}

/// Hub side: keeps every peer's picture of who is in the room current.
///
/// Mentions of any machine's agents are routed by the hub, but a peer's browser
/// can only offer names it has been told. So the hub tells every peer: at once
/// when someone joins, leaves or reports a new roster, and otherwise whenever a
/// periodic check finds a change — the hub's own agents start and stop too.
pub async fn run_directory(state: Arc<AppState>) {
    let mut sent: Vec<Member> = Vec::new();
    loop {
        let poked = tokio::select! {
            _ = state.rooms.directory_changed.notified() => true,
            _ = tokio::time::sleep(ROSTER_EVERY) => false,
        };
        if !state.rooms.has_peers() {
            sent.clear();
            continue;
        }
        let members = state.rooms.directory(local_roster(&state).await);
        // A poke means a peer may have just joined with nothing yet, so it
        // goes out even when the list itself is unchanged.
        if poked || members != sent {
            state.rooms.send_to_peers(&Frame::Directory { members: members.clone() });
            sent = members;
        }
    }
}

// --------------------------------------------------------------- peer side

/// Dials the hub and keeps the link up for the life of the process.
///
/// Backoff is exponential and capped: an AR headset waking on a flaky mesh must
/// reconnect quickly, and a hub that is down for an hour must not be hammered.
pub async fn run_link(state: Arc<AppState>) {
    let Some(hub) = state.config.room.hub.clone() else { return };
    let mut backoff = BACKOFF_MIN;

    loop {
        match connect_once(&state, &hub).await {
            Ok(()) => {
                log::warn!("peer link to {} closed; reconnecting", hub.url);
                backoff = BACKOFF_MIN;
            }
            Err(e) => {
                log::warn!("peer link to {}: {e}", hub.url);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
        state.rooms.clear_upstream();
        // Stale the moment the link drops: nobody listed can be reached.
        state.rooms.set_directory(Vec::new());
        tokio::time::sleep(backoff).await;
    }
}

async fn connect_once(state: &Arc<AppState>, hub: &crate::config::Hub) -> Result<(), String> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message as TMessage;

    let mut request = hub
        .url
        .as_str()
        .into_client_request()
        .map_err(|e| format!("bad hub url: {e}"))?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        format!("Bearer {}", hub.token)
            .parse()
            .map_err(|_| "hub token is not a valid header value".to_string())?,
    );

    let dial = tokio_tungstenite::connect_async_tls_with_config(
        request,
        None,
        false,
        tls(hub, state.config.room.ca.as_deref())?,
    );
    // Bounded: a hub whose kernel accepts the connection but whose process
    // never answers the upgrade would otherwise hold this dial, and with it
    // every reconnect, forever.
    let (socket, _) = tokio::time::timeout(LINK_IDLE, dial)
        .await
        .map_err(|_| "connect timed out".to_string())?
        .map_err(|e| format!("connect failed: {e}"))?;
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Frame>();

    log::info!("joined the room at {}", hub.url);
    // No roster yet: finding out which agents run means asking rmux, and a
    // slow or wedged daemon must not hold up joining. The task below announces
    // it moments later — mentions route by name meanwhile, not by roster.
    let _ = tx.send(Frame::Hello {
        peer: state.rooms.local_peer.clone(),
        agents: Vec::new(),
        since: state.rooms.last_seq(),
        epoch: Some(state.rooms.epoch()),
    });
    state.rooms.set_upstream(tx.clone());

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            let Ok(text) = serde_json::to_string(&frame) else { continue };
            if sink.send(TMessage::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Agents start and stop after the hello; without this the hub would offer
    // them for mentions as they were at connect time, forever.
    let roster = tokio::spawn({
        let (state, tx) = (state.clone(), tx.clone());
        async move {
            let mut announced = Vec::new();
            loop {
                let now = local_roster(&state).await;
                if now != announced {
                    if tx.send(Frame::Roster { agents: now.clone() }).is_err() {
                        break;
                    }
                    announced = now;
                }
                tokio::time::sleep(ROSTER_EVERY).await;
            }
        }
    });

    loop {
        let msg = match tokio::time::timeout(LINK_IDLE, stream.next()).await {
            Ok(Some(Ok(msg))) => msg,
            Ok(_) => break,
            // The hub pings every PING_EVERY; this long without even that is a
            // half-open socket, and only a fresh connection can tell whether
            // the hub is really there.
            Err(_) => {
                log::warn!("the hub at {} went silent", hub.url);
                break;
            }
        };
        let TMessage::Text(text) = msg else { continue };
        let Ok(frame) = serde_json::from_str::<Frame>(&text) else {
            log::warn!("hub sent an unparseable frame");
            continue;
        };
        match frame {
            Frame::Welcome { room, epoch, .. } => {
                state.rooms.adopt_hub_room(room);
                // Before the replay that follows, which is of the hub's log.
                if let Some(epoch) = epoch.filter(|e| *e != state.rooms.epoch()) {
                    state.rooms.restart_mirror(&epoch);
                }
            }
            Frame::Deliver { msg, replay } => {
                // Mirrored and queued here, in the order the hub sent them —
                // done in a spawned task, a later message could overtake it.
                state.rooms.ingest(msg.clone());
                if replay {
                    continue;
                }
                deliver(state, &msg);
            }
            Frame::RefRequest { req, r } => {
                let _ = tx.send(reply(req, resolve_ref_locally(state, &r)));
            }

            Frame::PeekRequest { req, agent, .. } => {
                let (state, tx) = (state.clone(), tx.clone());
                tokio::spawn(async move {
                    let _ = tx.send(reply(req, peek_local(&state, &agent, false).await));
                });
            }
            Frame::RefResponse { req, ok, err } => state.rooms.settle_ref(req, ok, err),
            Frame::Directory { members } => state.rooms.set_directory(members),
            other => log::warn!("hub sent an unexpected frame: {other:?}"),
        }
    }

    roster.abort();
    writer.abort();
    Ok(())
}

/// Trusts the hub's own certificate on top of the system roots.
///
/// `None` keeps the default connector, which is right for a hub whose
/// certificate chains to a public root. It is not right for the setup this
/// project recommends — `scripts/gen-phantom-cert.sh` issues a self-signed cert,
/// and dialing it fails with `UnknownIssuer`, which is to say the cross-machine
/// half of the room did not work at all against its own documented deployment.
///
/// Adding that one file as a trust anchor is narrower than skipping
/// verification: the hub still has to present that certificate.
fn tls(
    hub: &crate::config::Hub,
    mesh_ca: Option<&std::path::Path>,
) -> Result<Option<tokio_tungstenite::Connector>, String> {
    // `hub.ca` names one specific hub's issuer; `room.ca` is the mesh's, and is
    // the right answer when every host's certificate comes from one root —
    // then moving the hub changes nothing here.
    let Some(ca) = hub.ca.as_deref().or(mesh_ca) else {
        return Ok(None);
    };
    let mut roots = rustls::RootCertStore::empty();
    // Public roots stay in place: a mesh may have one hub on a real cert and
    // another self-signed, and the peer should reach both.
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        let _ = roots.add(cert);
    }

    let pem = std::fs::read(ca).map_err(|e| format!("reading {}: {e}", ca.display()))?;
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
        let cert = cert.map_err(|e| format!("parsing {}: {e}", ca.display()))?;
        roots
            .add(cert)
            .map_err(|e| format!("trusting {}: {e}", ca.display()))?;
        added += 1;
    }
    // Silently trusting nothing would look like a working config that always
    // fails the handshake — the hardest kind of misconfiguration to find.
    if added == 0 {
        return Err(format!("{} holds no certificate", ca.display()));
    }

    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(Some(tokio_tungstenite::Connector::Rustls(Arc::new(config))))
}

/// Hands a message to the agents this machine owns. The same on a hub and on a
/// peer: whichever machine runs the agent is the one that delivers to it.
///
/// Called in the order messages arrive, and quick: the assistant answers in a
/// task of its own, and every CLI agent gets the message through its queue —
/// one worker per agent, so it receives them one at a time and in order,
/// rather than two racing to be typed in, or the second landing in the task
/// the first has just started.
fn deliver(state: &Arc<AppState>, msg: &RoomMsg) {
    let assistant = state.config.room.assistant.as_ref().map(|a| a.name.as_str());
    let mut asks_assistant = false;
    for target in state.rooms.local_targets(msg) {
        if Some(target) == assistant {
            asks_assistant = true;
        } else {
            state.rooms.enqueue(state, target, msg.clone());
        }
    }
    if asks_assistant {
        // It reads attachments from wherever they live, so it gets the
        // message as sent, not the copy rewritten to this machine's cache.
        tokio::spawn(super::assistant::answer(state.clone(), msg.clone()));
    }
}

/// Drains one agent's queue for the life of the process.
async fn agent_worker(state: Arc<AppState>, agent: String, mut queue: mpsc::UnboundedReceiver<RoomMsg>) {
    while let Some(msg) = queue.recv().await {
        let msg = localize(&state, msg).await;
        if deliver_to(&state, &agent, &msg, DELIVER_WAIT).await {
            state.room_rmux.settle(&agent, SETTLE).await;
        }
    }
}

/// The message with its references pulled down onto this machine, so what the
/// agent is told to read is a path that exists here.
async fn localize(state: &Arc<AppState>, mut msg: RoomMsg) -> RoomMsg {
    let refs = std::mem::take(&mut msg.refs);
    for r in refs {
        match cache_remote_ref(state, &r).await {
            Ok(path) => msg.refs.push(Ref::Handoff { peer: state.rooms.local_peer.clone(), file: path }),
            Err(e) => {
                log::warn!("fetching {}: {e}", describe(&r));
                msg.refs.push(r);
            }
        }
    }
    msg
}

/// Fetches a reference owned by another machine and writes it under
/// `.phantom/room-cache/<peer>/`, returning the path to hand the agent.
async fn cache_remote_ref(state: &Arc<AppState>, r: &Ref) -> Result<String, String> {
    let Ref::Handoff { peer, file } = r else {
        // Blobs are bytes, and the room's frames are text. Cross-machine blob
        // fetching needs its own transfer path; until then the agent gets the
        // reference described rather than a broken local path.
        return Err("only handoff references cross machines today".into());
    };
    if peer == &state.rooms.local_peer {
        return Ok(file.clone());
    }
    // Checked here as well as on posting: whoever relays the message — a hub
    // this machine merely trusts to sequence — must not be able to name a
    // directory like `../../.config` and have its content written there.
    if !super::is_plain_name(peer) {
        return Err(format!("bad peer name {peer:?}"));
    }
    let content = state.rooms.request_ref(r.clone()).await?;
    let root = workspace_root(state)?;
    let dir = root.join(".phantom/room-cache").join(peer);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    // `file` came off the network, so it is only ever a leaf name here.
    let leaf = std::path::Path::new(file)
        .file_name()
        .ok_or_else(|| format!("bad reference name {file:?}"))?;
    let path = dir.join(leaf);
    std::fs::write(&path, content).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(phantom_core::path::to_canon(&path))
}

// ------------------------------------------------------------------ shared

fn reply(req: u64, answer: Result<String, String>) -> Frame {
    match answer {
        Ok(content) => Frame::RefResponse { req, ok: Some(content), err: None },
        Err(e) => Frame::RefResponse { req, ok: None, err: Some(e) },
    }
}

/// What `peer/agent`'s screen shows, from this machine or from the one it runs
/// on. `trusted` is someone logged in to this machine's own web UI, who may
/// look at its agents whatever `room.peek` says; it means nothing elsewhere.
pub(super) async fn peek(
    state: &Arc<AppState>,
    peer: &str,
    agent: &str,
    trusted: bool,
) -> Result<String, String> {
    if peer == state.rooms.local_peer {
        return peek_local(state, agent, trusted).await;
    }
    state.rooms.request_peek(peer, agent).await
}

async fn peek_local(state: &Arc<AppState>, agent: &str, trusted: bool) -> Result<String, String> {
    if !trusted && !state.config.room.peek {
        return Err(format!(
            "{} does not show its agents' screens (room.peek is off)",
            state.rooms.local_peer
        ));
    }
    let mut screen = state.room_rmux.peek(agent).await?;
    if screen.len() > MAX_SCREEN {
        // The bottom is where an agent's latest output is.
        let cut = screen.ceil_char_boundary(screen.len() - MAX_SCREEN);
        screen.replace_range(..cut, "…");
    }
    Ok(screen)
}

/// Serves a reference, from this machine or by asking the machine that owns it.
pub(super) async fn resolve_ref(state: &Arc<AppState>, r: Ref) -> Result<String, String> {
    match &r {
        Ref::Handoff { peer, .. } | Ref::Blob { peer, .. }
            if peer != &state.rooms.local_peer =>
        {
            state.rooms.request_ref(r).await
        }
        _ => resolve_ref_locally(state, &r),
    }
}

/// Reads a reference this machine owns.
///
/// The only readable surface is the workspace's handoff directory, through the
/// same traversal guard the orchestrator's `read_handoff` tool uses. There is no
/// arbitrary-path variant, so joining a room never grants a remote file read.
///
/// The owning peer is checked rather than ignored: answering someone else's
/// reference with this machine's same-named file would be worse than an error.
fn resolve_ref_locally(state: &Arc<AppState>, r: &Ref) -> Result<String, String> {
    let Ref::Handoff { peer, file } = r else {
        return Err("only handoff references can be served".into());
    };
    if peer != &state.rooms.local_peer {
        return Err(format!("{peer} owns that file, not {}", state.rooms.local_peer));
    }
    let root = workspace_root(state)?;
    crate::session::read_handoff_file(&root, file)
}

fn workspace_root(state: &Arc<AppState>) -> Result<std::path::PathBuf, String> {
    state
        .config
        .room
        .workspace
        .clone()
        .ok_or_else(|| "set `room.workspace` to share files in the room".to_string())
}

fn describe(r: &Ref) -> String {
    match r {
        Ref::Handoff { peer, file } => format!("handoff {peer}:{file}"),
        Ref::Blob { peer, id } => format!("blob {peer}:{id}"),
    }
}

/// This machine's own agents from the configured workspace, each marked with
/// whether its session is live — for the roster it announces upstream and the
/// one a room client is shown.
pub(super) async fn local_roster(state: &Arc<AppState>) -> Vec<RemoteAgent> {
    // The built-in assistant is always there to ask, workspace or not.
    let mut roster: Vec<RemoteAgent> = state
        .config
        .room
        .assistant
        .iter()
        .map(|a| RemoteAgent {
            name: a.name.clone(),
            role: "assistant".into(),
            running: true,
            state: state.rooms.assistant_state().map(str::to_string),
        })
        .collect();
    let Some(root) = state.config.room.workspace.as_deref() else {
        return roster;
    };
    let Ok(agents) = crate::agents::load(root) else {
        return roster;
    };
    // `agents::load` leaves `running` false for the caller to fill in; skip
    // that and every remote agent reads as stopped.
    let live: std::collections::HashSet<String> = state
        .room_rmux
        .list()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.name)
        .collect();
    roster.extend(agents.into_iter().map(|a| {
        let session = format!("phantom-agent-{}", a.name);
        RemoteAgent {
            running: live.contains(&session),
            state: state.agents.state(&session).map(str::to_string),
            name: a.name,
            role: a.role,
        }
    }));
    roster
}

/// A fixed-window rate limiter. Coarse on purpose: the job is to stop a flood,
/// not to meter precisely, and a window counter costs one integer.
pub(crate) struct Budget {
    limit: u32,
    used: u32,
    window_start: std::time::Instant,
}

impl Budget {
    pub(crate) fn new(limit: u32) -> Self {
        Self { limit, used: 0, window_start: std::time::Instant::now() }
    }

    pub(crate) fn allow(&mut self) -> bool {
        if self.window_start.elapsed() >= Duration::from_secs(60) {
            self.window_start = std::time::Instant::now();
            self.used = 0;
        }
        self.used += 1;
        self.used <= self.limit
    }
}

impl Rooms {
    pub(super) fn attach_peer(
        &self,
        name: &str,
        tx: mpsc::UnboundedSender<Frame>,
        agents: Vec<RemoteAgent>,
    ) {
        self.peers
            .lock()
            .expect("peers poisoned")
            .insert(name.to_string(), super::PeerLink { tx, agents });
        self.directory_changed.notify_one();
    }

    /// Forgets a peer's link, but only if it is still the one `tx` feeds. A
    /// peer that reconnects before the hub notices its old socket died has
    /// already replaced the link, and the old socket's teardown must not take
    /// the new one with it — the peer would stay connected and hear nothing.
    pub(super) fn detach_peer(&self, name: &str, tx: &mpsc::UnboundedSender<Frame>) {
        let mut peers = self.peers.lock().expect("peers poisoned");
        if peers.get(name).is_some_and(|link| link.tx.same_channel(tx)) {
            peers.remove(name);
            self.directory_changed.notify_one();
        }
    }

    /// Puts `msg` on `agent`'s queue, starting its worker on first use (or
    /// again, should the last one have ended).
    pub(super) fn enqueue(&self, state: &Arc<AppState>, agent: &str, msg: RoomMsg) {
        let mut queues = self.queues.lock().expect("queues poisoned");
        let msg = match queues.get(agent) {
            Some(queue) => match queue.send(msg) {
                Ok(()) => return,
                Err(mpsc::error::SendError(msg)) => msg,
            },
            None => msg,
        };
        let (queue, rx) = mpsc::unbounded_channel();
        let _ = queue.send(msg);
        queues.insert(agent.to_string(), queue);
        tokio::spawn(agent_worker(state.clone(), agent.to_string(), rx));
    }

    pub(super) fn has_peers(&self) -> bool {
        !self.peers.lock().expect("peers poisoned").is_empty()
    }

    /// Hub side: this server's own agents plus every joined peer's, in a
    /// stable order so an unchanged room compares equal from one check to
    /// the next.
    pub(super) fn directory(&self, local: Vec<RemoteAgent>) -> Vec<Member> {
        let mut members: Vec<Member> = local
            .into_iter()
            .map(|agent| Member { peer: self.local_peer.clone(), agent })
            .collect();
        members.extend(
            self.remote_agents().into_iter().map(|(peer, agent)| Member { peer, agent }),
        );
        members.sort_by(|a, b| (&a.peer, &a.agent.name).cmp(&(&b.peer, &b.agent.name)));
        members
    }

    /// Peer side: adopts the hub's directory.
    pub(super) fn set_directory(&self, members: Vec<Member>) {
        *self.directory.lock().expect("directory poisoned") = members;
    }

    /// Peers `msg` mentions that aren't joined — nobody will deliver to them.
    /// Hub only: a peer knows nothing of who else is connected.
    pub(super) fn absent_peers(&self, msg: &RoomMsg) -> Vec<String> {
        let peers = self.peers.lock().expect("peers poisoned");
        let mut absent: Vec<String> = Vec::new();
        for who in &msg.mentions {
            let gone = who.peer != self.local_peer && !peers.contains_key(&who.peer);
            if gone && !absent.contains(&who.peer) {
                absent.push(who.peer.clone());
            }
        }
        absent
    }

    pub(super) fn send_to_peers(&self, frame: &Frame) {
        for link in self.peers.lock().expect("peers poisoned").values() {
            let _ = link.tx.send(frame.clone());
        }
    }

    pub(super) fn set_upstream(&self, tx: mpsc::UnboundedSender<Frame>) {
        *self.upstream.lock().expect("upstream poisoned") = Some(tx);
    }

    pub(super) fn clear_upstream(&self) {
        *self.upstream.lock().expect("upstream poisoned") = None;
    }

    /// Asks whoever owns `r` for its content.
    ///
    /// A peer asks its hub; the hub asks the peer that owns it. Times out rather
    /// than waiting forever — the machine holding the file may have gone away
    /// mid-request, and an injection is waiting on this.
    pub async fn request_ref(&self, r: Ref) -> Result<String, String> {
        let owner = match &r {
            Ref::Handoff { peer, .. } | Ref::Blob { peer, .. } => peer.clone(),
        };
        self.ask(&owner, |req| Frame::RefRequest { req, r }).await
    }

    /// Asks the machine `agent` runs on what its screen shows.
    pub(super) async fn request_peek(&self, peer: &str, agent: &str) -> Result<String, String> {
        let (owner, agent) = (peer.to_string(), agent.to_string());
        self.ask(peer, |req| Frame::PeekRequest { req, peer: owner, agent }).await
    }

    /// Sends a request to `owner` — through the hub from a peer, straight to
    /// that peer's link from the hub — and waits for its [`Frame::RefResponse`].
    async fn ask(&self, owner: &str, request: impl FnOnce(u64) -> Frame) -> Result<String, String> {
        let req = self.next_ref_req.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending_refs.lock().expect("refs poisoned").insert(req, tx);

        let frame = request(req);
        let sent = match self.upstream.lock().expect("upstream poisoned").as_ref() {
            // A peer routes everything through the hub, which owns the topology.
            Some(up) => up.send(frame).is_ok(),
            None => match self.peers.lock().expect("peers poisoned").get(owner) {
                Some(link) => link.tx.send(frame).is_ok(),
                None => false,
            },
        };
        if !sent {
            self.pending_refs.lock().expect("refs poisoned").remove(&req);
            return Err(format!("{owner} is not in the room"));
        }

        match tokio::time::timeout(REF_TIMEOUT, rx).await {
            Ok(Ok(answer)) => answer,
            _ => {
                self.pending_refs.lock().expect("refs poisoned").remove(&req);
                Err(format!("{owner} did not answer in time"))
            }
        }
    }

    pub(super) fn settle_ref(&self, req: u64, ok: Option<String>, err: Option<String>) {
        let Some(tx) = self.pending_refs.lock().expect("refs poisoned").remove(&req) else {
            return; // already timed out
        };
        let _ = tx.send(match ok {
            Some(content) => Ok(content),
            None => Err(err.unwrap_or_else(|| "reference unavailable".into())),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn room_config(name: &str, peer: &str) -> crate::config::Room {
        crate::config::Room {
            name: name.into(),
            peer: peer.into(),
            ..Default::default()
        }
    }

    #[test]
    fn budget_caps_a_flood_and_recovers_next_window() {
        let mut b = Budget::new(2);
        assert!(b.allow());
        assert!(b.allow());
        assert!(!b.allow(), "third post in the window must be dropped");
        // Fake the window rolling over rather than sleeping a minute.
        b.window_start = std::time::Instant::now() - Duration::from_secs(61);
        assert!(b.allow());
    }

    #[test]
    fn a_joined_peers_agents_show_up_in_the_roster() {
        let rooms = Rooms::new(&room_config("roster-test", "alpha"));
        let (tx, _rx) = mpsc::unbounded_channel();
        rooms.attach_peer(
            "b",
            tx.clone(),
            vec![RemoteAgent {
                name: "agy".into(),
                role: "frontend".into(),
                running: true,
                state: None,
            }],
        );
        let remote = rooms.remote_agents();
        assert_eq!(remote.len(), 1);
        assert_eq!(remote[0].0, "b");
        assert_eq!(remote[0].1.name, "agy");

        // Leaving must take its agents with it, or the UI keeps offering a
        // mention that can no longer be delivered.
        rooms.detach_peer("b", &tx);
        assert!(rooms.remote_agents().is_empty());
    }

    #[test]
    fn a_dead_sockets_teardown_leaves_the_reconnected_link_alone() {
        let rooms = Rooms::new(&room_config("relink-test", "alpha"));
        let agy = || {
            vec![RemoteAgent { name: "agy".into(), role: String::new(), running: true, state: None }]
        };
        let (old, _old_rx) = mpsc::unbounded_channel();
        let (new, _new_rx) = mpsc::unbounded_channel();
        rooms.attach_peer("b", old.clone(), agy());
        // The peer reconnected while the hub still held its half-open socket.
        rooms.attach_peer("b", new.clone(), agy());

        rooms.detach_peer("b", &old);
        assert_eq!(rooms.remote_agents().len(), 1, "the live link must survive");
        rooms.detach_peer("b", &new);
        assert!(rooms.remote_agents().is_empty());
    }

    #[test]
    fn a_peer_forwards_its_posts_instead_of_sequencing_them() {
        let rooms = Rooms::new(&crate::config::Room {
            hub: Some(crate::config::Hub {
                url: "wss://hub.invalid/room/ws".into(),
                token: "t".into(),
                ca: None,
            }),
            ..room_config("upstream-test", "b")
        });
        let (tx, mut rx) = mpsc::unbounded_channel();
        rooms.set_upstream(tx);
        // `None` is the contract: the hub owns `seq`, so inventing one here
        // would put a message in the log that no other machine agrees with.
        assert!(matches!(
            rooms.post(Participant::human("b"), "hi".into(), vec![]),
            Ok(None)
        ));
        assert!(matches!(rx.try_recv(), Ok(Frame::Post { .. })));
    }

    fn agent(name: &str) -> RemoteAgent {
        RemoteAgent { name: name.into(), role: String::new(), running: true, state: None }
    }

    #[test]
    fn a_peer_can_offer_every_machines_agents_but_its_own() {
        let rooms = Rooms::new(&crate::config::Room {
            hub: Some(crate::config::Hub {
                url: "wss://hub.invalid/room/ws".into(),
                token: "t".into(),
                ca: None,
            }),
            ..room_config("directory-peer-test", "b")
        });
        rooms.set_directory(vec![
            Member { peer: "alpha".into(), agent: agent("claude") },
            Member { peer: "b".into(), agent: agent("agy") },
            Member { peer: "c".into(), agent: agent("codex") },
        ]);
        let ids: Vec<String> =
            rooms.remote_agents().into_iter().map(|(p, a)| format!("{p}/{}", a.name)).collect();
        // `b/agy` is this machine's own, listed from its own roster instead.
        assert_eq!(ids, vec!["alpha/claude", "c/codex"]);
    }

    #[test]
    fn the_hub_directory_is_complete_and_stably_ordered() {
        let rooms = Rooms::new(&room_config("directory-hub-test", "alpha"));
        let (b, _b_rx) = mpsc::unbounded_channel();
        let (c, _c_rx) = mpsc::unbounded_channel();
        rooms.attach_peer("c", c, vec![agent("x")]);
        rooms.attach_peer("b", b, vec![agent("agy"), agent("aaa")]);
        let ids = |members: Vec<Member>| -> Vec<String> {
            members.into_iter().map(|m| format!("{}/{}", m.peer, m.agent.name)).collect()
        };
        let first = ids(rooms.directory(vec![agent("claude")]));
        assert_eq!(first, vec!["alpha/claude", "b/aaa", "b/agy", "c/x"]);
        // Same room, same list: otherwise the periodic check would re-send it
        // every time the peer map happened to iterate differently.
        assert_eq!(ids(rooms.directory(vec![agent("claude")])), first);
    }

    #[test]
    fn directory_frames_round_trip() {
        let f = Frame::Directory {
            members: vec![Member {
                peer: "b".into(),
                agent: RemoteAgent { state: Some("working".into()), ..agent("agy") },
            }],
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""peer":"b","name":"agy""#), "{json}");
        let Frame::Directory { members } = serde_json::from_str(&json).unwrap() else {
            panic!("not a directory frame");
        };
        assert_eq!(members[0].agent.state.as_deref(), Some("working"));
    }

    #[test]
    fn a_peer_on_another_log_starts_over_and_one_on_this_log_catches_up() {
        // Same log: pick up after the last message it has.
        assert_eq!(replay_plan("e1", Some("e1"), Some(41)), (Some(41), false));
        // The hub's log was replaced, or the peer has none: from the start,
        // and set the old copy aside — its 41 is in another log's numbers.
        assert_eq!(replay_plan("e2", Some("e1"), Some(41)), (None, true));
        // An older peer says nothing about epochs; it keeps what it had.
        assert_eq!(replay_plan("e2", None, Some(41)), (Some(41), false));
    }

    #[test]
    fn frames_round_trip() {
        let f = Frame::Post {
            agent: Some("agy".into()),
            text: "done".into(),
            refs: vec![Ref::Handoff { peer: "b".into(), file: "api.md".into() }],
            notice: false,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"t\":\"post\""));
        let back: Frame = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, Frame::Post { agent: Some(a), .. } if a == "agy"));
    }

    #[test]
    fn a_missing_optional_field_still_parses() {
        // The peer on the other end may be an older build; a Post without
        // `refs` or `agent` must not drop the whole link.
        let back: Frame = serde_json::from_str(r#"{"t":"post","text":"hi"}"#).unwrap();
        assert!(matches!(back, Frame::Post { agent: None, refs, .. } if refs.is_empty()));
    }
}
