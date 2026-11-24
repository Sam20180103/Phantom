//! The room: one shared, append-only message log that people and CLI agents
//! both post into.
//!
//! Why this exists: the orchestration built so far (`Phantom-任务清单.md` §二 P1)
//! passes content between agents through `.phantom/handoff/*.md` and triggers
//! them with `nudge`. Both assume one filesystem, i.e. one machine. The room
//! lifts "who is doing what, and who needs whose output" off the filesystem and
//! onto a log every participant can see, so the same workflow spans hosts.
//!
//! Deliberate shape:
//!
//! * **Messages carry references, not content.** A [`Ref`] names a handoff file
//!   or a blob; the receiving side reads it with its own file tools. Keeps the
//!   log small and keeps "the file is the protocol" intact.
//! * **Mentioning an agent injects into its session.** `@claude` resolves to a
//!   participant and the message is handed to [`crate::rmux_pty::RmuxState::nudge`], which
//!   already waits for the agent's hook to report idle. No new delivery
//!   machinery.
//! * **No new wire protocol.** Posting is an ordinary `invoke`; the fan-out
//!   rides the generic `event` frame the frontend already listens on.

pub mod assistant;
pub mod attach;
pub mod client;
pub mod commands;
pub mod http;
pub mod peer;
// Named `store`, not `log`: a module called `log` inside this one would
// shadow the logging crate for every `log::warn!` below it.
mod store;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, mpsc, oneshot};

pub use peer::{Frame, Member, RemoteAgent};
pub use store::RoomLog;

/// How many messages a subscriber may fall behind before it is told to resync.
/// A lagging receiver gets `RecvError::Lagged`, which the socket task turns
/// into a gap notice; the client then refills with `room_history`.
const BROADCAST_CAP: usize = 256;

/// Name for this server's own participants when no peer name is configured.
pub const LOCAL_PEER: &str = "local";

/// Longest message body accepted. The room carries pointers, not payloads — a
/// wall of text belongs in a handoff file that the message references.
const MAX_TEXT: usize = 8 * 1024;

/// References per message, and the longest name one may carry. Plenty for real
/// handoffs; small enough that one post can't fan out into a fetch storm on
/// every machine it mentions.
const MAX_REFS: usize = 16;
const MAX_REF_NAME: usize = 255;

/// Agent-to-agent handoffs in a row, with no person in between, before the
/// room stops delivering them. Agents are told how to answer, so they answer:
/// two that keep mentioning each other would otherwise go on — and bill their
/// owners — forever.
const AGENT_CHAIN_LIMIT: u32 = 10;

/// Why [`Rooms::post`] turned a message away.
#[derive(Debug, PartialEq, Eq)]
pub enum Refused {
    /// Empty, too long, or carrying too many or oversized references.
    Invalid(&'static str),
    /// This server is a peer and its link to the hub is down. Sequencing the
    /// message here instead would hand out a `seq` the hub hands out too, and
    /// the replay on reconnect would then skip the hub's copy for good.
    HubUnreachable,
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Invalid(why) => f.write_str(why),
            Refused::HubUnreachable => f.write_str("not connected to the room's hub; try again shortly"),
        }
    }
}

/// Who said it, or who is being addressed.
///
/// `agent: None` is a human on that host. The pair is the room's whole identity
/// model — there is no per-user login (see `auth.rs`: one shared credential), so
/// a "human" is really "someone at this server's browser".
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Participant {
    pub peer: String,
    #[serde(default)]
    pub agent: Option<String>,
}

impl Participant {
    pub fn human(peer: impl Into<String>) -> Self {
        Self { peer: peer.into(), agent: None }
    }

    pub fn agent(peer: impl Into<String>, agent: impl Into<String>) -> Self {
        Self { peer: peer.into(), agent: Some(agent.into()) }
    }

    /// `peer/agent`, or `peer/human`. Used in injected text and in the UI, so
    /// it must stay stable and unambiguous.
    pub fn display(&self) -> String {
        format!("{}/{}", self.peer, self.agent.as_deref().unwrap_or("human"))
    }
}

/// A pointer to content that lives outside the message.
///
/// The two variants are the *entire* surface a remote participant can read:
/// the workspace's handoff directory and the blob store. There is deliberately
/// no "arbitrary path" variant — that would hand every machine on the mesh a
/// remote file read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Ref {
    Handoff { peer: String, file: String },
    Blob { peer: String, id: String },
}

impl Ref {
    fn describe(&self) -> String {
        match self {
            Ref::Handoff { peer, file } => format!("handoff {peer}:{file}"),
            Ref::Blob { peer, id } => format!("blob {peer}:{id}"),
        }
    }
}

/// Something a participant said, or a notice the room posted about itself.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgKind {
    #[default]
    Say,
    /// Posted by the server a participant is on, not by the participant: a
    /// delivery that failed, a peer that isn't there. Never delivered to
    /// anyone, so a notice can't set off another notice.
    Notice,
}

impl MsgKind {
    fn is_say(&self) -> bool {
        *self == MsgKind::Say
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomMsg {
    /// Assigned by this server, monotonic. Clients dedupe and detect gaps on it.
    pub seq: u64,
    /// Unix milliseconds.
    pub ts: i64,
    pub from: Participant,
    #[serde(default)]
    pub mentions: Vec<Participant>,
    pub text: String,
    #[serde(default)]
    pub refs: Vec<Ref>,
    /// Omitted for an ordinary message, so what older builds read is unchanged.
    #[serde(default, skip_serializing_if = "MsgKind::is_say")]
    pub kind: MsgKind,
}

/// One joined peer server: where to send it frames, and what it can do.
struct PeerLink {
    tx: mpsc::UnboundedSender<Frame>,
    agents: Vec<RemoteAgent>,
}

pub struct Rooms {
    pub name: String,
    /// This server's own peer name. Messages it originates are stamped with it,
    /// and a bare `@agent` mention resolves to it.
    pub local_peer: String,
    /// Per-agent tokens and where to post them, so this server can tell an
    /// agent how to answer and can put the credential in its environment.
    agent_tokens: std::collections::BTreeMap<String, String>,
    post_url: String,
    /// This binary's own path, for the reply command handed to agents. Resolved
    /// once: an agent is told to run the very server it is talking to.
    exe: Option<String>,
    /// The room's name as the hub calls it, learned on joining. `name` above is
    /// only this server's local label — it picks the log file, and on a peer it
    /// would otherwise show up in injected messages and in the UI as a second
    /// name for the one room everybody is in.
    hub_room: Mutex<Option<String>>,
    log: Mutex<RoomLog>,
    tx: broadcast::Sender<RoomMsg>,
    /// Hub side: peer servers currently joined, by name.
    peers: Mutex<HashMap<String, PeerLink>>,
    /// Configured with a `hub:`. Fixed for the life of the process: a peer
    /// whose link is down is still a peer, not a hub with a log of its own.
    joins_hub: bool,
    /// Peer side: the link to the hub, while it is up. See [`Rooms::post`],
    /// which forwards through it instead of appending.
    upstream: Mutex<Option<mpsc::UnboundedSender<Frame>>>,
    /// Peer side: everyone in the room, as the hub last described it. Only the
    /// hub hears every peer's roster; without this a peer's browser could offer
    /// mentions of nobody but its own agents.
    directory: Mutex<Vec<Member>>,
    /// Hub side: set when who is in the room may have changed, so the
    /// directory goes out now rather than at the next periodic check.
    directory_changed: tokio::sync::Notify,
    /// Hub side: agent messages addressing agents since a person last spoke.
    agent_chain: AtomicU32,
    /// One delivery queue per agent on this machine, each drained by a single
    /// worker — see [`peer::deliver`] — so an agent gets its messages one at a
    /// time, in the order they came.
    queues: Mutex<HashMap<String, mpsc::UnboundedSender<RoomMsg>>>,
    /// Questions the built-in assistant is answering right now. It has no
    /// session or hooks, so this is what the roster reports as its state.
    assistant_busy: AtomicU32,
    /// In-flight [`Frame::RefRequest`]s awaiting their answer.
    pending_refs: Mutex<HashMap<u64, oneshot::Sender<Result<String, String>>>>,
    next_ref_req: AtomicU64,
}

impl Rooms {
    pub fn new(config: &crate::config::Room) -> Self {
        Self::with_log(config, default_log_path(&config.name))
    }

    fn with_log(config: &crate::config::Room, path: std::path::PathBuf) -> Self {
        let name = config.name.clone();
        let (tx, _) = broadcast::channel(BROADCAST_CAP);
        Self {
            local_peer: config.peer.clone(),
            agent_tokens: config.agent_tokens.clone(),
            post_url: config.post_url.clone(),
            exe: std::env::current_exe()
                .ok()
                .map(|p| p.display().to_string()),
            hub_room: Mutex::new(None),
            log: Mutex::new(RoomLog::open(path)),
            tx,
            name,
            peers: Mutex::new(HashMap::new()),
            joins_hub: config.hub.is_some(),
            upstream: Mutex::new(None),
            directory: Mutex::new(Vec::new()),
            directory_changed: tokio::sync::Notify::new(),
            agent_chain: AtomicU32::new(0),
            queues: Mutex::new(HashMap::new()),
            assistant_busy: AtomicU32::new(0),
            pending_refs: Mutex::new(HashMap::new()),
            next_ref_req: AtomicU64::new(1),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<RoomMsg> {
        self.tx.subscribe()
    }

    /// Records a message and fans it out. Returns it with its assigned `seq` so
    /// the poster can correlate its own echo.
    ///
    /// Mentions are parsed from `text` here rather than trusted from the
    /// caller, and the limits are checked here too, so the browser, the
    /// `phantom say` CLI, a room client and a peer link all get identical
    /// treatment from identical input.
    ///
    /// `Ok(None)` when this server is a peer: the message went upstream and
    /// the hub owns sequencing, so the authoritative copy arrives moments later
    /// as an ordinary broadcast rather than being invented here.
    pub fn post(
        &self,
        from: Participant,
        text: String,
        refs: Vec<Ref>,
    ) -> Result<Option<RoomMsg>, Refused> {
        let text = validate(&text, &refs)?;
        if self.joins_hub {
            let frame = Frame::Post { agent: from.agent, text, refs, notice: false };
            return if self.send_upstream(frame) { Ok(None) } else { Err(Refused::HubUnreachable) };
        }
        let mut mentions = parse_mentions(&text, &self.local_peer);
        let tripped = self.hold_runaway_chain(&from, &mut mentions);
        let msg = self.record(from, mentions, text, refs, MsgKind::Say);
        if tripped {
            self.notice(format!(
                "agents have handed work to each other {AGENT_CHAIN_LIMIT} times in a row with \
                 no person in between, so their mentions are no longer being delivered. \
                 Mention an agent yourself to carry on."
            ));
        }
        Ok(Some(msg))
    }

    /// Counts agent-to-agent handoffs, and once they run past the limit stops
    /// delivering them by dropping the message's mentions — the text is still
    /// logged, so everyone can see what was said. Any person speaking resets
    /// the count. True the moment the limit is crossed, so it is said once.
    fn hold_runaway_chain(&self, from: &Participant, mentions: &mut Vec<Participant>) -> bool {
        if from.agent.is_none() {
            self.agent_chain.store(0, Ordering::Relaxed);
            return false;
        }
        if mentions.is_empty() {
            return false;
        }
        let chain = self.agent_chain.fetch_add(1, Ordering::Relaxed) + 1;
        if chain <= AGENT_CHAIN_LIMIT {
            return false;
        }
        mentions.clear();
        chain == AGENT_CHAIN_LIMIT + 1
    }

    /// Posts a notice from this server into the room, for everyone to see.
    ///
    /// Best effort: a peer whose hub is unreachable drops it, since the
    /// failure it reports is the same one keeping it from the room.
    pub fn notice(&self, text: String) {
        if self.joins_hub {
            let frame = Frame::Post { agent: None, text, refs: Vec::new(), notice: true };
            if !self.send_upstream(frame) {
                log::debug!("dropping a room notice: the hub is unreachable");
            }
            return;
        }
        self.notice_from(Participant::human(&self.local_peer), text);
    }

    /// Hub side: records a notice some server posted.
    pub(crate) fn notice_from(&self, from: Participant, text: String) {
        self.record(from, Vec::new(), text, Vec::new(), MsgKind::Notice);
    }

    /// Sequences, stores, and hands one message to every browser here and
    /// every joined peer. Hub only.
    ///
    /// All under the log lock, so the order messages go out in is the order
    /// they were numbered in — a peer mirrors what arrives, and a notice sent
    /// ahead of the message it is about would be stored ahead of it too.
    fn record(
        &self,
        from: Participant,
        mentions: Vec<Participant>,
        text: String,
        refs: Vec<Ref>,
        kind: MsgKind,
    ) -> RoomMsg {
        let mut log = self.log.lock().expect("room log poisoned");
        let msg = RoomMsg { seq: log.next_seq(), ts: now_ms(), from, mentions, text, refs, kind };
        log.append(msg.clone());
        // Err just means nobody is subscribed right now. The log already has it.
        let _ = self.tx.send(msg.clone());
        self.send_to_peers(&Frame::Deliver { msg: msg.clone(), replay: false });
        msg
    }

    /// Hands a frame to the hub link. False when there is no link to hand it to.
    fn send_upstream(&self, frame: Frame) -> bool {
        let up = self.upstream.lock().expect("upstream poisoned");
        up.as_ref().is_some_and(|up| up.send(frame).is_ok())
    }

    /// Records a message the hub assigned. The peer side's counterpart to
    /// [`Self::post`]: same log, same broadcast, but the `seq` is not ours.
    pub fn ingest(&self, msg: RoomMsg) {
        self.log.lock().expect("room log poisoned").append(msg.clone());
        let _ = self.tx.send(msg);
    }

    pub fn is_peer(&self) -> bool {
        self.joins_hub
    }

    /// Every remote participant, for the roster the UI offers for `@` mentions.
    ///
    /// A hub knows its peers' rosters first-hand; a peer knows what the hub's
    /// directory told it, less its own agents, which it lists itself.
    pub fn remote_agents(&self) -> Vec<(String, RemoteAgent)> {
        if self.joins_hub {
            return self
                .directory
                .lock()
                .expect("directory poisoned")
                .iter()
                .filter(|m| m.peer != self.local_peer)
                .map(|m| (m.peer.clone(), m.agent.clone()))
                .collect();
        }
        self.peers
            .lock()
            .expect("peers poisoned")
            .iter()
            .flat_map(|(name, link)| {
                link.agents.iter().map(move |a| (name.clone(), a.clone()))
            })
            .collect()
    }

    /// What to call this room in anything a person or an agent reads.
    pub fn title(&self) -> String {
        self.hub_room
            .lock()
            .expect("hub room poisoned")
            .clone()
            .unwrap_or_else(|| self.name.clone())
    }

    /// Records the name the hub gave. Sent on every join, so a reconnect after
    /// the hub was renamed corrects itself.
    pub fn adopt_hub_room(&self, room: String) {
        *self.hub_room.lock().expect("hub room poisoned") = Some(room);
    }

    /// `NAME=VALUE` pairs giving an agent's session the credential it needs to
    /// answer, so the reply command never has to carry a token in its argv —
    /// which would put it in the agent's scrollback and its context.
    ///
    /// Empty for an agent with no configured token: it can be addressed, it
    /// just has no way to answer, and telling it to run a command that 401s
    /// would be worse than silence.
    pub fn agent_env(&self, agent: &str) -> Vec<String> {
        match self.agent_tokens.get(agent) {
            Some(token) => vec![
                format!("PHANTOM_ROOM_TOKEN={token}"),
                format!("PHANTOM_ROOM_URL={}", self.post_url),
            ],
            None => Vec::new(),
        }
    }

    /// The one-line "here is how you answer" appended to an injected message.
    ///
    /// Agents have no memory of the room between messages, so this rides along
    /// every time rather than being explained once.
    pub fn reply_hint(&self, agent: &str) -> Option<String> {
        if !self.agent_tokens.contains_key(agent) {
            return None;
        }
        let exe = self.exe.as_deref()?;
        Some(format!(
            "to reply in the room, run: {exe} say \"<your answer>\" \
             (add --ref <file> to attach one from .phantom/handoff/)"
        ))
    }

    pub fn last_seq(&self) -> Option<u64> {
        self.log.lock().expect("room log poisoned").last_seq()
    }

    /// Which log the numbers belong to; see [`RoomLog`]. Sent with history and
    /// with every message, so anyone holding messages from a replaced log
    /// knows to start over rather than mix the two.
    pub fn epoch(&self) -> String {
        self.log.lock().expect("room log poisoned").epoch().to_string()
    }

    /// Peer side: the hub's log is not the one this machine was mirroring.
    pub(crate) fn restart_mirror(&self, epoch: &str) {
        if self.log.lock().expect("room log poisoned").restart_as(epoch) {
            log::warn!("the hub's room log changed; this machine's copy was set aside");
        }
    }

    pub fn history(&self, since: Option<u64>) -> Vec<RoomMsg> {
        self.log.lock().expect("room log poisoned").since(since)
    }

    /// The built-in assistant's state, in the tracker's vocabulary.
    pub fn assistant_state(&self) -> Option<&'static str> {
        (self.assistant_busy.load(Ordering::Relaxed) > 0).then_some("working")
    }

    /// Marks the assistant busy until the returned guard is dropped, and says
    /// so to peers now rather than at the next directory check.
    pub(crate) fn assistant_working(&self) -> AssistantBusy<'_> {
        self.assistant_busy.fetch_add(1, Ordering::Relaxed);
        self.directory_changed.notify_one();
        AssistantBusy(self)
    }

    /// The agents on this server that `msg` addresses, never counting whoever
    /// sent it.
    pub fn local_targets<'a>(&'a self, msg: &'a RoomMsg) -> impl Iterator<Item = &'a str> + 'a {
        // A notice carries no mentions of its own; the guard is for a hostile
        // peer that sends one anyway.
        let mentions = if msg.kind == MsgKind::Notice { &[][..] } else { &msg.mentions[..] };
        mentions
            .iter()
            .filter(move |who| who.peer == self.local_peer && **who != msg.from)
            .filter_map(|who| who.agent.as_deref())
    }
}

/// Held while the assistant answers; see [`Rooms::assistant_working`].
pub(crate) struct AssistantBusy<'a>(&'a Rooms);

impl Drop for AssistantBusy<'_> {
    fn drop(&mut self) {
        self.0.assistant_busy.fetch_sub(1, Ordering::Relaxed);
        self.0.directory_changed.notify_one();
    }
}

/// The text to record, or why the message can't be posted at all.
fn validate(text: &str, refs: &[Ref]) -> Result<String, Refused> {
    let text = text.trim();
    if text.is_empty() {
        return Err(Refused::Invalid("empty message"));
    }
    if text.len() > MAX_TEXT {
        return Err(Refused::Invalid("message too long; put the detail in a handoff file"));
    }
    if refs.len() > MAX_REFS {
        return Err(Refused::Invalid("too many references"));
    }
    let oversized = refs.iter().any(|r| match r {
        Ref::Handoff { peer, file } => peer.len() > MAX_REF_NAME || file.len() > MAX_REF_NAME,
        Ref::Blob { peer, id } => peer.len() > MAX_REF_NAME || id.len() > MAX_REF_NAME,
    });
    if oversized {
        return Err(Refused::Invalid("reference name too long"));
    }
    let bad_owner = refs.iter().any(|r| match r {
        Ref::Handoff { peer, .. } | Ref::Blob { peer, .. } => !is_plain_name(peer),
    });
    if bad_owner {
        return Err(Refused::Invalid("a reference must name a peer, not a path"));
    }
    Ok(text.to_string())
}

/// One ordinary path component: not empty, not `.` or `..`, no separator.
/// A peer name becomes a directory on every machine that caches a file it
/// owns, so anything else would let a reference write outside that directory.
pub fn is_plain_name(name: &str) -> bool {
    let mut parts = std::path::Path::new(name).components();
    matches!(
        (parts.next(), parts.next()),
        (Some(std::path::Component::Normal(part)), None) if part == name
    )
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn default_log_path(room: &str) -> std::path::PathBuf {
    dirs::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("phantom/rooms")
        .join(format!("{room}.jsonl"))
}

/// `agent` or `peer/agent`, read exactly as the same words would be as a
/// mention. `None` for anything that isn't one.
pub fn parse_target(target: &str, local_peer: &str) -> Option<(String, String)> {
    let who = parse_mentions(&format!("@{}", target.trim()), local_peer).into_iter().next()?;
    Some((who.peer, who.agent?))
}

/// Pulls `@agent` and `@peer/agent` out of message text.
///
/// A bare name means "an agent on this server" — the common case, and the one a
/// CLI typing into the room can produce without knowing the topology. Duplicates
/// are collapsed so a message naming someone twice injects into them once.
fn parse_mentions(text: &str, local_peer: &str) -> Vec<Participant> {
    let mut out: Vec<Participant> = Vec::new();
    for raw in text.split_whitespace().filter_map(|w| w.strip_prefix('@')) {
        // Trailing punctuation ("@claude," at the end of a sentence) is not part
        // of the name, but '/' is kept so a half-typed "@b/" is rejected below
        // rather than silently collapsing to the local agent "b".
        let token =
            raw.trim_end_matches(|c: char| !c.is_alphanumeric() && c != '_' && c != '/');
        if token.is_empty() {
            continue;
        }
        let who = match token.split_once('/') {
            Some((peer, agent)) if !peer.is_empty() && !agent.is_empty() => {
                Participant::agent(peer, agent)
            }
            // A trailing or leading slash is a typo, not an address.
            Some(_) => continue,
            None => Participant::agent(local_peer, token),
        };
        if !out.contains(&who) {
            out.push(who);
        }
    }
    out
}

/// What an agent actually sees typed into its terminal.
///
/// One line, because it is injected into a live CLI: the agent is mid-session
/// and this has to read as an instruction, not as a protocol frame. References
/// are named, not inlined — the agent opens them with its own file tools.
pub fn render_injection(room: &str, msg: &RoomMsg) -> String {
    let mut line = format!("[room:{room}] {}: {}", msg.from.display(), msg.text);
    if !msg.refs.is_empty() {
        let refs: Vec<String> = msg.refs.iter().map(Ref::describe).collect();
        line.push_str(" · refs: ");
        line.push_str(&refs.join(", "));
    }
    // Typed into a terminal, a control character is a keystroke, not text: an
    // ESC can end a bracketed paste early and a CR submits whatever preceded
    // it, both on behalf of whoever wrote the message. As spaces they are
    // harmless, and the line stays one line.
    line.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// Types a message into one agent on this server, the way its queue delivers
/// each: started first under `room.auto-start`, then waited on until it is
/// ready for input. A failure comes back as a notice — whoever asked may be on another
/// machine, and that is the only way they learn the work never started. True
/// if the message went in.
pub async fn deliver_to(
    state: &crate::AppState,
    agent: &str,
    msg: &RoomMsg,
    wait: std::time::Duration,
) -> bool {
    let (rooms, rmux) = (&state.rooms, &state.room_rmux);
    let me = format!("{}/{agent}", rooms.local_peer);
    if let (true, Some(workspace)) = (state.config.room.auto_start, state.config.room.workspace.as_deref()) {
        if let Err(e) = rmux.wake_agent(agent, workspace, &state.registry).await {
            log::warn!("starting {me} for the room failed: {e}");
            rooms.notice(format!("could not start {me}: {e}"));
            return false;
        }
    }
    // Carry the way back. Delivery is otherwise one-way: the agent reads the
    // message, does the work, and answers in its own terminal, where nobody in
    // the room ever sees it.
    let text = render_injection(&rooms.title(), msg);
    let line = match rooms.reply_hint(agent) {
        Some(hint) => format!("{text} · {hint}"),
        None => text,
    };
    let failure = match rmux.deliver_room_message(agent, &line, wait).await {
        Ok(true) => return true,
        Ok(false) => format!(
            "{me} stayed busy for {} minutes, so this was not typed in; mention it again when it is free",
            wait.as_secs() / 60
        ),
        Err(e) => format!("could not deliver to {me}: {e}"),
    };
    log::warn!("room delivery: {failure}");
    rooms.notice(failure);
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_mention_is_local_qualified_one_is_not() {
        let m = parse_mentions("@claude and @b/agy please", "local");
        assert_eq!(m, vec![Participant::agent("local", "claude"), Participant::agent("b", "agy")]);
    }

    #[test]
    fn punctuation_is_trimmed_and_duplicates_collapse() {
        // "@claude," must address claude, and naming someone twice must not
        // inject into them twice.
        let m = parse_mentions("@claude, ping @claude!", "local");
        assert_eq!(m, vec![Participant::agent("local", "claude")]);
    }

    #[test]
    fn non_mentions_are_ignored() {
        assert!(parse_mentions("mail me at a@b.com or @ nothing", "local")
            .iter()
            .all(|p| p.agent.as_deref() != Some("b")));
        assert!(parse_mentions("@ @/ @x/", "local").is_empty());
    }

    #[test]
    fn injection_names_refs_without_inlining_them() {
        let msg = RoomMsg {
            seq: 1,
            ts: 0,
            from: Participant::agent("b", "agy"),
            mentions: vec![],
            text: "api is ready".into(),
            refs: vec![Ref::Handoff { peer: "b".into(), file: "api.md".into() }],
            kind: Default::default(),
        };
        let line = render_injection("main", &msg);
        assert_eq!(line, "[room:main] b/agy: api is ready · refs: handoff b:api.md");
        // One line: it is typed into a live TUI.
        assert!(!line.contains('\n'));
    }

    fn rooms_with_token() -> Rooms {
        let mut agent_tokens = std::collections::BTreeMap::new();
        agent_tokens.insert("claude".to_string(), "tok-a".to_string());
        Rooms::new(&crate::config::Room {
            name: "t".into(),
            peer: "local".into(),
            post_url: "https://host:8099".into(),
            agent_tokens,
            ..Default::default()
        })
    }

    #[test]
    fn a_configured_agent_gets_its_credential_by_environment() {
        let rooms = rooms_with_token();
        let env = rooms.agent_env("claude");
        assert!(env.contains(&"PHANTOM_ROOM_TOKEN=tok-a".to_string()));
        assert!(env.contains(&"PHANTOM_ROOM_URL=https://host:8099".to_string()));
        // The token must never travel in the command the agent is told to run:
        // argv reaches shell history, `ps`, and the agent's own scrollback.
        assert!(!rooms.reply_hint("claude").unwrap().contains("tok-a"));
    }

    #[test]
    fn an_agent_with_no_token_is_told_nothing() {
        let rooms = rooms_with_token();
        // It can still be addressed; it just has no way to answer, and telling
        // it to run a command that would 401 is worse than saying nothing.
        assert!(rooms.agent_env("agy").is_empty());
        assert_eq!(rooms.reply_hint("agy"), None);
    }

    #[test]
    fn a_delivered_message_carries_the_way_back() {
        let rooms = rooms_with_token();
        let msg = RoomMsg {
            seq: 1,
            ts: 0,
            from: Participant::human("local"),
            mentions: vec![Participant::agent("local", "claude")],
            text: "check the logs".into(),
            refs: Vec::new(),
            kind: Default::default(),
        };
        let line = format!(
            "{} · {}",
            render_injection(&rooms.name, &msg),
            rooms.reply_hint("claude").expect("hint")
        );
        assert!(line.contains("check the logs"));
        assert!(line.contains("say"));
        // Still one line: this is typed into a live TUI.
        assert!(!line.contains('\n'));
    }

    #[test]
    fn a_peer_shows_the_hubs_room_name_not_its_own() {
        let rooms = Rooms::new(&crate::config::Room {
            name: "whatever-i-called-my-log".into(),
            peer: "b".into(),
            ..Default::default()
        });
        // Before joining there is nothing better to show than the local label.
        assert_eq!(rooms.title(), "whatever-i-called-my-log");

        rooms.adopt_hub_room("main".into());
        // One room, one name: an injected message must not invent a second.
        assert_eq!(rooms.title(), "main");
        // The log file keeps the local name — it is this machine's own storage.
        assert_eq!(rooms.name, "whatever-i-called-my-log");
    }

    #[test]
    fn every_entry_point_gets_the_same_limits() {
        assert_eq!(validate("  hi  ", &[]), Ok("hi".to_string()));
        assert_eq!(validate(" \n ", &[]), Err(Refused::Invalid("empty message")));
        assert!(validate(&"x".repeat(MAX_TEXT + 1), &[]).is_err());

        let r = |file: String| Ref::Handoff { peer: "b".into(), file };
        let many: Vec<Ref> = (0..=MAX_REFS).map(|i| r(format!("{i}.md"))).collect();
        assert!(validate("hi", &many).is_err());
        assert!(validate("hi", &[r("x".repeat(MAX_REF_NAME + 1))]).is_err());
        assert!(validate("hi", &[r("api.md".into())]).is_ok());
    }

    #[test]
    fn a_reference_owner_is_a_name_never_a_path() {
        for ok in ["b", "alpha-2", "héllo"] {
            assert!(is_plain_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "../x", "a/b", "/abs", "a/"] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
        let r = Ref::Handoff { peer: "../../.config".into(), file: "x".into() };
        assert!(validate("hi", &[r]).is_err());
    }

    #[test]
    fn an_injection_cannot_carry_keystrokes() {
        let msg = RoomMsg {
            seq: 1,
            ts: 0,
            from: Participant::human("b"),
            mentions: vec![],
            // Ends a bracketed paste, submits, then types a second line.
            text: "hi\x1b[201~\rrm -rf ~\nand\tmore\u{9b}2J".into(),
            refs: vec![Ref::Handoff { peer: "b".into(), file: "a\rb.md".into() }],
            kind: Default::default(),
        };
        let line = render_injection("main", &msg);
        assert!(!line.chars().any(char::is_control), "{line:?}");
        assert!(line.contains("rm -rf ~ and more"));
    }

    #[test]
    fn a_peer_whose_link_is_down_refuses_rather_than_sequencing_locally() {
        let rooms = Rooms::new(&crate::config::Room {
            name: "phantom-test-peer-down".into(),
            peer: "b".into(),
            hub: Some(crate::config::Hub {
                url: "wss://hub.invalid/room/ws".into(),
                token: "t".into(),
                ca: None,
            }),
            ..Default::default()
        });
        let before = rooms.last_seq();
        // A local seq here would collide with one the hub hands out, and the
        // replay on reconnect would then skip the hub's message for good.
        assert!(matches!(
            rooms.post(Participant::human("b"), "hi".into(), vec![]),
            Err(Refused::HubUnreachable)
        ));
        assert_eq!(rooms.last_seq(), before);
        assert!(rooms.is_peer(), "still a peer while disconnected");
    }

    #[test]
    fn local_targets_are_this_servers_agents_minus_the_sender() {
        let rooms = rooms_with_token();
        let msg = RoomMsg {
            seq: 1,
            ts: 0,
            from: Participant::agent("local", "claude"),
            mentions: vec![
                Participant::agent("local", "claude"),
                Participant::agent("local", "agy"),
                Participant::agent("b", "codex"),
                Participant::human("local"),
            ],
            text: "@claude @agy @b/codex".into(),
            refs: Vec::new(),
            kind: Default::default(),
        };
        assert_eq!(rooms.local_targets(&msg).collect::<Vec<_>>(), vec!["agy"]);
    }

    fn hub_with_temp_log(dir: &tempfile::TempDir) -> Rooms {
        let config = crate::config::Room { peer: "alpha".into(), ..Default::default() };
        Rooms::with_log(&config, dir.path().join("room.jsonl"))
    }

    #[test]
    fn a_notice_is_logged_for_everyone_but_delivered_to_nobody() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = hub_with_temp_log(&dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        rooms.attach_peer("b", tx, Vec::new());

        rooms.notice("could not deliver to alpha/claude: not running".into());
        let logged = rooms.history(None);
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].kind, MsgKind::Notice);
        // Relayed so a peer's browser shows it too.
        assert!(matches!(rx.try_recv(), Ok(Frame::Deliver { msg, .. }) if msg.kind == MsgKind::Notice));

        // Even a notice that names someone must not reach them: that is how
        // one failed delivery would turn into an endless chain of them.
        let mut noisy = logged[0].clone();
        noisy.mentions = vec![Participant::agent("alpha", "claude")];
        assert_eq!(rooms.local_targets(&noisy).count(), 0);
    }

    #[test]
    fn agents_mentioning_each_other_forever_are_stopped_until_a_person_speaks() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = hub_with_temp_log(&dir);
        let claude = || Participant::agent("alpha", "claude");
        let post = |from: Participant, text: &str| rooms.post(from, text.into(), vec![]).unwrap().unwrap();

        for _ in 0..AGENT_CHAIN_LIMIT {
            assert_eq!(post(claude(), "@agy thanks!").mentions.len(), 1);
        }
        let held = post(claude(), "@agy thanks again!");
        assert!(held.mentions.is_empty(), "past the limit, nobody is injected");
        let notices = rooms.history(None).iter().filter(|m| m.kind == MsgKind::Notice).count();
        assert_eq!(notices, 1, "and the room says why, once");
        post(claude(), "@agy still here");
        assert_eq!(
            rooms.history(None).iter().filter(|m| m.kind == MsgKind::Notice).count(),
            1
        );

        // A person speaking is what hands control back.
        post(Participant::human("alpha"), "ok, one more round");
        assert_eq!(post(claude(), "@agy go").mentions.len(), 1);
    }

    #[test]
    fn peers_receive_everything_in_the_order_it_was_numbered() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = hub_with_temp_log(&dir);
        let (tx, mut rx) = mpsc::unbounded_channel();
        rooms.attach_peer("b", tx, Vec::new());
        // Enough to trip the chain limit, whose notice is numbered right after
        // the message that tripped it and must not overtake it.
        for _ in 0..=AGENT_CHAIN_LIMIT {
            rooms.post(Participant::agent("alpha", "claude"), "@agy hi".into(), vec![]).unwrap();
        }
        let mut seqs = Vec::new();
        while let Ok(Frame::Deliver { msg, .. }) = rx.try_recv() {
            seqs.push(msg.seq);
        }
        assert_eq!(seqs.len() as u32, AGENT_CHAIN_LIMIT + 2, "every message plus the notice");
        assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");
    }

    #[test]
    fn a_mention_of_a_machine_not_in_the_room_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let rooms = hub_with_temp_log(&dir);
        let (tx, _rx) = mpsc::unbounded_channel();
        rooms.attach_peer("b", tx, Vec::new());
        let msg = RoomMsg {
            seq: 1,
            ts: 0,
            from: Participant::human("alpha"),
            mentions: vec![
                Participant::agent("alpha", "claude"),
                Participant::agent("b", "agy"),
                Participant::agent("c", "x"),
                Participant::agent("c", "y"),
            ],
            text: String::new(),
            refs: Vec::new(),
            kind: Default::default(),
        };
        assert_eq!(rooms.absent_peers(&msg), vec!["c".to_string()]);
    }

    #[test]
    fn an_ordinary_message_reads_and_writes_as_before() {
        // Older builds neither send nor expect `kind`.
        let old = r#"{"seq":3,"ts":0,"from":{"peer":"b","agent":null},"text":"hi"}"#;
        let msg: RoomMsg = serde_json::from_str(old).unwrap();
        assert_eq!(msg.kind, MsgKind::Say);
        assert!(!serde_json::to_string(&msg).unwrap().contains("kind"));
    }

    #[test]
    fn the_assistant_reads_as_working_exactly_while_it_answers() {
        let rooms = rooms_with_token();
        assert_eq!(rooms.assistant_state(), None);
        {
            let _first = rooms.assistant_working();
            let _second = rooms.assistant_working();
            assert_eq!(rooms.assistant_state(), Some("working"));
            drop(_first);
            assert_eq!(rooms.assistant_state(), Some("working"), "one question still open");
        }
        assert_eq!(rooms.assistant_state(), None);
    }

    #[test]
    fn a_peek_target_reads_like_a_mention() {
        assert_eq!(parse_target("claude", "alpha"), Some(("alpha".into(), "claude".into())));
        assert_eq!(parse_target("b/agy", "alpha"), Some(("b".into(), "agy".into())));
        assert_eq!(parse_target("b/", "alpha"), None);
        assert_eq!(parse_target("", "alpha"), None);
    }

    #[test]
    fn display_distinguishes_human_from_agent() {
        assert_eq!(Participant::human("local").display(), "local/human");
        assert_eq!(Participant::agent("b", "agy").display(), "b/agy");
    }
}
