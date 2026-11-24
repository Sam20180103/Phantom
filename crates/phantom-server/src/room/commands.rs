//! The room's `invoke` surface: post, history, roster.
//!
//! Async because the roster lists rmux sessions, so it hangs off the same
//! dispatch step as the other rmux-backed commands rather than the blocking one.
//! Posting itself never waits on delivery — see [`peer::fanout`].

use serde::Deserialize;
use serde_json::Value;

use super::{peer, Participant, Ref};
use crate::session::Connection;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostArgs {
    text: String,
    #[serde(default)]
    refs: Vec<Ref>,
    /// Text to keep as files here and reference — see [`super::attach`].
    #[serde(default)]
    attach: Vec<super::attach::Attachment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryArgs {
    #[serde(default)]
    since: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeekArgs {
    /// `agent` or `peer/agent`.
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RosterArgs {
    cwd: String,
    #[serde(default)]
    workspace: Option<phantom_core::workspace::WorkspaceEnv>,
}

impl Connection {
    pub async fn dispatch_room(&self, cmd: &str, args: Value) -> Option<Result<Value, String>> {
        Some(match cmd {
            // A browser post is always from the human at this server.
            "room_post" => match parse::<PostArgs>(args) {
                Ok(a) => self.room_post(a),
                Err(e) => Err(e),
            },
            // Backfill after a reconnect: the client passes the last seq it saw.
            "room_history" => match parse::<HistoryArgs>(args) {
                Ok(a) => Ok(serde_json::json!({
                    "room": self.rooms.title(),
                    "localPeer": self.rooms.local_peer,
                    "epoch": self.rooms.epoch(),
                    "messages": self.rooms.history(a.since),
                })),
                Err(e) => Err(e),
            },
            // Who can be mentioned. Today that is this server's configured
            // agents plus its human; peers add to it in the cross-machine step.
            "room_roster" => match parse::<RosterArgs>(args) {
                Ok(a) => self.room_roster(a.cwd, a.workspace).await,
                Err(e) => Err(e),
            },
            // A look at an agent's screen. Logged in to this machine, its own
            // agents are an open terminal away anyway, so no `room.peek` here;
            // another machine's decide for themselves.
            "room_peek" => match parse::<PeekArgs>(args) {
                Ok(a) => match super::parse_target(&a.target, &self.rooms.local_peer) {
                    Some((peer, agent)) => peer::peek(&self.app, &peer, &agent, true)
                        .await
                        .map(Value::String),
                    None => Err(format!("{:?} is not an agent", a.target)),
                },
                Err(e) => Err(e),
            },
            _ => return None,
        })
    }

    fn room_post(&self, a: PostArgs) -> Result<Value, String> {
        let workspace = self.app.config.room.workspace.as_deref();
        let mut refs = a.refs;
        refs.extend(super::attach::store(workspace, &self.rooms.local_peer, a.attach)?);
        let from = Participant::human(&self.rooms.local_peer);
        // `None` means this server is a peer: the post went to the hub, and
        // the authoritative copy arrives as a broadcast.
        match self.rooms.post(from, a.text, refs).map_err(|e| e.to_string())? {
            Some(msg) => {
                peer::fanout(&self.app, &msg);
                Ok(serde_json::json!(msg))
            }
            None => Ok(Value::Null),
        }
    }

    async fn room_roster(
        &self,
        cwd: String,
        workspace: Option<phantom_core::workspace::WorkspaceEnv>,
    ) -> Result<Value, String> {
        // The browser knows which folder is open, so it says; a remote client
        // has no cwd and falls back to the configured workspace instead.
        let mut agents = match self.agent_list(cwd, workspace).await? {
            Value::Array(agents) => agents,
            _ => Vec::new(),
        };
        // The built-in assistant has no session, but it can always be asked.
        if let Some(a) = &self.app.config.room.assistant {
            agents.insert(0, serde_json::json!({
                "name": a.name,
                "role": "assistant",
                "running": true,
                "state": self.rooms.assistant_state(),
            }));
        }
        Ok(roster_entries(&self.rooms, &self.app.agents, &agents))
    }
}

/// Builds the roster the UI offers for `@` completion: this server's human,
/// its configured agents, and every agent its peers have announced.
///
/// Shared so the browser and a remote client can never be offered different
/// sets of people to address.
///
/// `state` is what each agent's hooks last reported, so whoever is about to
/// hand one work can see whether it is busy, waiting on a person, or no longer
/// running its CLI at all.
pub(crate) fn roster_entries(
    rooms: &super::Rooms,
    tracker: &crate::rmux_pty::AgentTracker,
    local_agents: &[Value],
) -> Value {
    let peer = &rooms.local_peer;
    let mut entries = vec![serde_json::json!({
        "id": Participant::human(peer).display(),
        "peer": peer,
        "agent": Value::Null,
        "role": "human",
        "running": true,
    })];
    for a in local_agents {
        let name = a.get("name").and_then(Value::as_str).unwrap_or_default();
        entries.push(serde_json::json!({
            "id": Participant::agent(peer, name).display(),
            "peer": peer,
            "agent": name,
            "role": a.get("role").cloned().unwrap_or(Value::Null),
            "running": a.get("running").cloned().unwrap_or(Value::Bool(false)),
            // Whatever the entry already says — the assistant reports its
            // own — else what the agent's hooks last said.
            "state": a
                .get("state")
                .filter(|s| !s.is_null())
                .cloned()
                .unwrap_or_else(|| tracker.state(&format!("phantom-agent-{name}")).into()),
        }));
    }
    // Peers announce their own rosters; a mention of `b/agy` is routed by the
    // hub, so the UI has to be able to offer it.
    for (peer_name, a) in rooms.remote_agents() {
        entries.push(serde_json::json!({
            "id": Participant::agent(&peer_name, &a.name).display(),
            "peer": peer_name,
            "agent": a.name,
            "role": a.role,
            "running": a.running,
            "state": a.state,
        }));
    }
    Value::Array(entries)
}

fn parse<T: serde::de::DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))
}
