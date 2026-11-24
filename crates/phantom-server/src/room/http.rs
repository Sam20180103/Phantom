//! `POST /room/post` — how a CLI agent speaks into the room.
//!
//! Separate from the WebSocket surface on purpose. An agent is a subscription
//! CLI running in a terminal, not a browser: the cheapest thing it can do is one
//! `curl`, and that has to work from any language and any machine. So this is a
//! plain authenticated POST, and both `phantom say` and the MCP tool are thin
//! wrappers over it rather than separate implementations.
//!
//! It sits *outside* the session-cookie middleware because agents don't have a
//! browser session. Its own credential is a per-agent bearer token from the
//! config's `room:` block, which is also the agent's identity in the room.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::Json;
use serde::{Deserialize, Serialize};

use super::{peer, Participant, Ref, Refused};
use crate::AppState;

#[derive(Deserialize)]
pub struct PostBody {
    text: String,
    #[serde(default)]
    refs: Vec<Ref>,
    /// Files in this machine's `.phantom/handoff/` to attach — what `say --ref`
    /// sends. The server names the machine, so an agent needn't know it.
    #[serde(default)]
    handoffs: Vec<String>,
}

#[derive(Serialize)]
pub struct PostResponse {
    /// Absent when this server is a peer — the hub assigns the sequence, and
    /// the caller does not wait for it.
    seq: Option<u64>,
    from: String,
}

pub async fn post(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<PostBody>,
) -> Result<Json<PostResponse>, (StatusCode, String)> {
    // No detail until the caller is known to be one of our agents: a prober
    // learns whether it is authenticated, and nothing about who else is.
    let unauthorized = || (StatusCode::UNAUTHORIZED, String::new());
    let token = bearer(&headers).ok_or_else(unauthorized)?;
    let agent = state
        .config
        .room
        .agent_for_token(token)
        .ok_or_else(unauthorized)?
        .to_string();
    // From here on the reason is the agent's to read — `say` prints it, and
    // "no such handoff file" is something it can fix.
    let bad_request = |why: String| (StatusCode::BAD_REQUEST, why);

    let mut refs = body.refs;
    for file in body.handoffs {
        // Checked now, while the agent is there to hear about it; the machine
        // fetching it later could only report a broken reference.
        let root = state.config.room.workspace.as_deref().ok_or_else(|| {
            bad_request("this machine shares no files; set room.workspace".into())
        })?;
        crate::session::read_handoff_file(root, &file).map_err(bad_request)?;
        refs.push(Ref::Handoff { peer: state.rooms.local_peer.clone(), file });
    }

    let from = Participant::agent(&state.rooms.local_peer, &agent);
    let display = from.display();
    // An agent posting can mention another agent, so this is a real fan-out
    // path, not just a log write.
    let seq = match state.rooms.post(from, body.text, refs) {
        Ok(Some(msg)) => {
            peer::fanout(&state, &msg);
            Some(msg.seq)
        }
        Ok(None) => None,
        Err(e @ Refused::Invalid(_)) => return Err(bad_request(e.to_string())),
        // `say` exits non-zero on this, so the agent learns its answer did
        // not land instead of believing it did.
        Err(e @ Refused::HubUnreachable) => {
            return Err((StatusCode::SERVICE_UNAVAILABLE, e.to_string()))
        }
    };

    Ok(Json(PostResponse { seq, from: display }))
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, value.parse().unwrap());
        h
    }

    #[test]
    fn reads_a_bearer_token() {
        assert_eq!(bearer(&headers("Bearer tok-a")), Some("tok-a"));
    }

    #[test]
    fn rejects_other_schemes_and_empty_tokens() {
        assert_eq!(bearer(&headers("Basic tok-a")), None);
        assert_eq!(bearer(&headers("Bearer ")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }
}
