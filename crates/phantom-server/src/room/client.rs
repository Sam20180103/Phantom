//! `GET /room/client` — a device that wants the room and nothing else.
//!
//! The browser served by this server already reaches the room over `/ws`, using
//! the login cookie. That credential is the wrong one to hand a phone or a
//! laptop across the mesh: `/ws` also opens PTYs and reads files, so "let me
//! read the chat" would mean "here is a shell on my machine".
//!
//! So this endpoint exists with its own class of credential — a client token,
//! which reaches the room's three commands and nothing else. No PTY, no
//! filesystem, no orchestrator.
//!
//! **Authentication is the first frame, not a header.** A browser's WebSocket
//! API cannot set `Authorization`, and Tauri's frontend is a browser; a token in
//! the URL would land in logs and history. So the client connects and
//! immediately sends `{"type":"auth","token":"…"}`. Anything else, or nothing at
//! all, and the socket closes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::Value;

use super::{peer, Participant, Ref};
use crate::AppState;

/// How long a fresh socket has to identify itself. Long enough for a slow link,
/// short enough that unauthenticated sockets can't accumulate.
const AUTH_TIMEOUT: Duration = Duration::from_secs(5);

/// Messages per minute from one client. A stolen token should not be able to
/// flood the log or nudge every agent in a loop.
const POST_BUDGET: u32 = 60;

/// How often a connected client is pinged and sent a keepalive.
///
/// Both, because they answer different questions. A browser answers a ping on
/// its own, which tells this server the client is still there; but a page's
/// script never sees pings, so the keepalive is what tells *it* the server
/// still is — a phone that slept wakes to a socket that looks open and hears
/// nothing, and without this would sit there, silently out of date.
const KEEPALIVE_EVERY: Duration = Duration::from_secs(20);

/// Silence after which a client is taken to be gone.
const CLIENT_IDLE: Duration = Duration::from_secs(60);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Auth { token: String },
    Invoke {
        req: u64,
        cmd: String,
        #[serde(default)]
        args: Value,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PostArgs {
    text: String,
    #[serde(default)]
    refs: Vec<Ref>,
    /// Text to keep as files on this machine and reference from the message —
    /// the only way a device with no files of its own can hand any over.
    #[serde(default)]
    attach: Vec<super::attach::Attachment>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeekArgs {
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryArgs {
    #[serde(default)]
    since: Option<u64>,
}

/// `GET /room` — the room for a device with nothing but a browser: a phone, a
/// borrowed laptop, anything without the desktop app.
///
/// One self-contained page, served without the login. It holds no secrets and
/// can do nothing until it is given a client token, which it sends in its
/// first frame to [`handler`] like any other room client — so it reaches the
/// room and nothing else. Its script renders every message as text, never as
/// markup; the policy below is the second line behind that.
pub async fn page() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
                 connect-src 'self'; img-src 'self' data:; base-uri 'none'; \
                 form-action 'none'; frame-ancestors 'none'",
            ),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        include_str!("page.html"),
    )
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state))
}

async fn serve(socket: WebSocket, state: Arc<AppState>) {
    let (mut sink, mut stream) = socket.split();

    let Some(device) = authenticate(&mut stream, &state).await else {
        // No reason given, on purpose: probing must not reveal which tokens or
        // device names exist.
        let _ = sink.send(Message::Close(None)).await;
        return;
    };
    log::info!("room client {device:?} connected");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut beat = tokio::time::interval(KEEPALIVE_EVERY);
        beat.tick().await; // the first tick is immediate; `ready` just went out
        loop {
            let frames = tokio::select! {
                text = rx.recv() => match text {
                    Some(text) => vec![Message::Text(text.into())],
                    None => break,
                },
                _ = beat.tick() => vec![
                    Message::Ping(Default::default()),
                    Message::Text(r#"{"type":"keepalive"}"#.into()),
                ],
            };
            for frame in frames {
                if sink.send(frame).await.is_err() {
                    return;
                }
            }
        }
    });
    let _ = tx.send(json_line(&serde_json::json!({ "type": "ready" })));

    let feed = tokio::spawn(room_feed(state.rooms.clone(), tx.clone()));
    let mut budget = peer::Budget::new(POST_BUDGET);

    loop {
        let text = match tokio::time::timeout(CLIENT_IDLE, stream.next()).await {
            Ok(Some(Ok(Message::Text(text)))) => text,
            Ok(Some(Ok(Message::Close(_)) | Err(_)) | None) => break,
            // A pong answering our ping: arriving at all was its job.
            Ok(Some(Ok(_))) => continue,
            Err(_) => {
                log::info!("room client {device:?} went silent; dropping it");
                break;
            }
        };
        let Ok(ClientFrame::Invoke { req, cmd, args }) = serde_json::from_str(&text) else {
            log::debug!("room client {device:?} sent a frame that is not an invoke");
            continue;
        };
        let result = dispatch(&state, &cmd, args, &mut budget).await;
        let reply = match result {
            Ok(value) => serde_json::json!({ "type": "result", "req": req, "ok": value }),
            Err(e) => {
                log::debug!("room client {device:?}: {cmd} failed: {e}");
                serde_json::json!({ "type": "result", "req": req, "err": e })
            }
        };
        if tx.send(json_line(&reply)).is_err() {
            break;
        }
    }

    feed.abort();
    writer.abort();
    log::info!("room client {device:?} disconnected");
}

/// Reads the first frame and checks it. Returns the device's name.
async fn authenticate(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
    state: &Arc<AppState>,
) -> Option<String> {
    let first = tokio::time::timeout(AUTH_TIMEOUT, stream.next()).await.ok()??;
    let Ok(Message::Text(text)) = first else {
        return None;
    };
    let Ok(ClientFrame::Auth { token }) = serde_json::from_str::<ClientFrame>(&text) else {
        return None;
    };
    state
        .config
        .room
        .client_for_token(token.trim())
        .map(str::to_string)
}

/// The whole command surface a client token reaches. Anything not listed is not
/// merely unauthorized — it does not exist here.
async fn dispatch(
    state: &Arc<AppState>,
    cmd: &str,
    args: Value,
    budget: &mut peer::Budget,
) -> Result<Value, String> {
    match cmd {
        "room_post" => {
            if !budget.allow() {
                return Err("too many messages; slow down".into());
            }
            let a: PostArgs =
                serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
            let mut refs = a.refs;
            refs.extend(super::attach::store(
                state.config.room.workspace.as_deref(),
                &state.rooms.local_peer,
                a.attach,
            )?);
            // A client posts as the human at this server, the same as its
            // browser does. The room has no per-person identity to give it.
            let from = Participant::human(&state.rooms.local_peer);
            match state.rooms.post(from, a.text, refs) {
                Ok(Some(msg)) => {
                    peer::fanout(state, &msg);
                    Ok(serde_json::json!(msg))
                }
                Ok(None) => Ok(Value::Null),
                Err(e) => Err(e.to_string()),
            }
        }
        "room_history" => {
            let a: HistoryArgs =
                serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
            Ok(serde_json::json!({
                "room": state.rooms.title(),
                "localPeer": state.rooms.local_peer,
                "epoch": state.rooms.epoch(),
                "messages": state.rooms.history(a.since),
            }))
        }
        "room_roster" => {
            // No cwd from a remote client, so the configured workspace is what
            // locates `agents.yaml`.
            let agents: Vec<Value> = peer::local_roster(state)
                .await
                .into_iter()
                .map(|a| serde_json::json!(a))
                .collect();
            Ok(super::commands::roster_entries(&state.rooms, &state.agents, &agents))
        }
        // A client is never trusted with a screen; each machine's `room.peek`
        // decides whether its agents can be looked at.
        "room_peek" => {
            let a: PeekArgs =
                serde_json::from_value(args).map_err(|e| format!("bad arguments: {e}"))?;
            let (peer, agent) = super::parse_target(&a.target, &state.rooms.local_peer)
                .ok_or_else(|| format!("{:?} is not an agent", a.target))?;
            peer::peek(state, &peer, &agent, false).await.map(Value::String)
        }
        other => Err(format!("unknown command: {other}")),
    }
}

/// Same shape the browser's socket pushes, so one frontend handles both.
async fn room_feed(
    rooms: Arc<super::Rooms>,
    tx: tokio::sync::mpsc::UnboundedSender<String>,
) {
    use tokio::sync::broadcast::error::RecvError;

    let mut rx = rooms.subscribe();
    loop {
        let payload = match rx.recv().await {
            Ok(msg) => serde_json::json!({ "kind": "message", "message": msg, "epoch": rooms.epoch() }),
            Err(RecvError::Lagged(n)) => serde_json::json!({ "kind": "lagged", "missed": n }),
            Err(RecvError::Closed) => break,
        };
        let frame = serde_json::json!({
            "type": "event",
            "event": "room:event",
            "payload": payload,
        });
        if tx.send(json_line(&frame)).is_err() {
            break;
        }
    }
}

fn json_line(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "{}".into())
}

#[cfg(test)]
mod tests {
    use axum::response::IntoResponse;

    #[tokio::test]
    async fn the_room_page_talks_to_the_client_endpoint_and_locks_itself_down() {
        let resp = super::page().await.into_response();
        let csp = resp.headers()[axum::http::header::CONTENT_SECURITY_POLICY].to_str().unwrap();
        assert!(csp.contains("connect-src 'self'") && csp.contains("frame-ancestors 'none'"));
        let page = include_str!("page.html");
        assert!(page.contains("/room/client"));
        // Messages are text: nothing that renders markup may touch them.
        assert!(!page.contains("innerHTML") && !page.contains("insertAdjacentHTML"));
    }
}
