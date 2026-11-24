//! The WebSocket loop. Split into a reader task (client → PTY) and a writer
//! task (PTY → client) so a slow client can never block a PTY thread.

use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;

use crate::protocol::{decode_pty_data, decode_stream_request, ClientMsg, ServerMsg};
use crate::config::PtyBackend;
use crate::session::{Connection, Outbound};
use crate::AppState;

pub async fn handler(ws: WebSocketUpgrade, State(state): State<Arc<AppState>>) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state))
}

async fn serve(socket: WebSocket, state: Arc<AppState>) {
    use futures_util::{SinkExt, StreamExt};

    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();
    let backend = state.config.pty_backend;
    let conn = Arc::new(Connection::new(&state, tx.clone()));

    // Fan the room's broadcast into this socket. Room posts arrive from other
    // browsers, from agents over HTTP, and later from peer servers, so the only
    // way this client learns about them is a server push — hence a task per
    // connection rather than anything on the request path.
    let room_feed = tokio::spawn(room_feed(state.rooms.clone(), tx.clone()));

    // Writer: the only task that touches the socket's send half.
    let writer = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let closing = matches!(&out, Outbound::Close);
            let mut permit = None;
            let msg = match out {
                Outbound::Binary(b) => Message::Binary(b.into()),
                Outbound::Text(t) => Message::Text(t.into()),
                Outbound::Close => Message::Close(None),
                Outbound::Stream(bytes, held) => {
                    permit = Some(held);
                    Message::Binary(bytes.into())
                }
            };
            if sink.send(msg).await.is_err() {
                break; // client went away
            }
            drop(permit);
            if closing {
                break;
            }
        }
    });

    while let Some(Ok(msg)) = stream.next().await {
        match msg {
            Message::Binary(bytes) if decode_stream_request(&bytes).is_some() => {
                let (endpoint, req, body) = decode_stream_request(&bytes).unwrap();
                conn.pty_stream.request(endpoint, req, body, tx.clone());
            }
            // Hot path: terminal input. Framed, not JSON — see protocol.rs.
            Message::Binary(bytes) => match decode_pty_data(&bytes) {
                Some((id, data)) => {
                    let result = match backend {
                        PtyBackend::Rmux => conn.rmux.write(id, data).await,
                        PtyBackend::Local => conn.pty.write(id, data),
                    };
                    if let Err(e) = result {
                        // Expected when the child just exited; the exit event
                        // is already on its way to the client.
                        log::debug!("pty write id={id} failed: {e}");
                    }
                }
                None => log::warn!("dropping malformed binary frame ({} bytes)", bytes.len()),
            },
            Message::Text(text) => {
                let parsed: ClientMsg = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(e) => {
                        log::warn!("dropping unparseable text frame: {e}");
                        continue;
                    }
                };
                let ClientMsg::Invoke { req, cmd, args } = parsed;
                let conn = conn.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    // Async network commands stream over `tx` keyed by `req`,
                    // so they run here rather than on the blocking pool. `None`
                    // falls through.
                    let result = conn.dispatch_pty_stream(&cmd, args.clone()).await;
                    let result = match result {
                        Some(result) => Some(result),
                        None => crate::net_commands::dispatch(&cmd, args.clone(), req, &tx, &conn.ai_stream).await,
                    };
                    // The rmux backend owns the PTY commands and is async; it
                    // returns `None` for everything else so the rest still goes
                    // through the blocking dispatch.
                    let result = match result {
                        Some(r) => Some(r),
                        // Orchestrator loop: async + streams steps by req, needs
                        // the connection's sessions, so it runs here too.
                        None => conn.dispatch_orchestrator(&cmd, args.clone(), req).await,
                    };
                    let result = match result {
                        Some(r) => Some(r),
                        // The room roster lists rmux sessions, so the room's
                        // commands are async like the rmux ones.
                        None => conn.dispatch_room(&cmd, args.clone()).await,
                    };
                    let result = match result {
                        Some(r) => Some(r),
                        None => match backend {
                            PtyBackend::Rmux => conn.dispatch_rmux_pty(&cmd, args.clone()).await,
                            PtyBackend::Local => None,
                        },
                    };
                    let result = match result {
                        Some(r) => r,
                        None => {
                            // Spawning a shell and touching the filesystem both
                            // block; keep them off the runtime's workers.
                            let conn = conn.clone();
                            let cmd = cmd.clone();
                            match tokio::task::spawn_blocking(move || conn.dispatch(&cmd, args))
                                .await
                            {
                                Ok(r) => r,
                                Err(e) => Err(format!("command panicked: {e}")),
                            }
                        }
                    };
                    let reply = match result {
                        Ok(value) => ServerMsg::ok(req, value),
                        Err(e) => {
                            log::warn!("{cmd} failed: {e}");
                            ServerMsg::err(req, e)
                        }
                    };
                    match serde_json::to_string(&reply) {
                        Ok(text) => {
                            let _ = tx.send(Outbound::Text(text));
                        }
                        Err(e) => log::error!("serializing reply to {cmd} failed: {e}"),
                    }
                });
            }
            Message::Close(_) => break,
            // Ping/Pong are handled by axum.
            _ => {}
        }
    }

    conn.ai_stream.shutdown();
    conn.pty_stream.disconnect();
    match backend {
        // Detach only: surviving a dropped client is why these live in rmux.
        PtyBackend::Rmux => {
            let detached = conn.rmux.detach_all().await;
            if detached > 0 {
                log::info!("connection closed, detached from {detached} rmux session(s)");
            }
        }
        // A local PTY dies with this process anyway, so leaving it running
        // would just strand a shell nobody can reach.
        PtyBackend::Local => {
            let reaped = conn.pty.close_all();
            if reaped > 0 {
                log::info!("connection closed, reaped {reaped} session(s)");
            }
        }
    }
    room_feed.abort();
    writer.abort();
}

/// Pushes every room message to one client as a generic `event` frame — the
/// same shape the filesystem watcher uses, so the frontend needs no new
/// protocol handling, just a `listen("room:event")`.
///
/// A client that falls behind [`broadcast`]'s buffer gets `Lagged` rather than
/// a silent gap. It is told how many it missed and refills with `room_history`;
/// dropping the connection instead would lose a terminal over a chat backlog.
async fn room_feed(rooms: Arc<crate::room::Rooms>, tx: tokio::sync::mpsc::UnboundedSender<Outbound>) {
    use tokio::sync::broadcast::error::RecvError;

    let mut rx = rooms.subscribe();
    loop {
        let payload = match rx.recv().await {
            // With the log's epoch: a client holding messages from a log that
            // has since been replaced refills rather than merging the two.
            Ok(msg) => serde_json::json!({ "kind": "message", "message": msg, "epoch": rooms.epoch() }),
            Err(RecvError::Lagged(n)) => {
                log::warn!("room feed lagged by {n} message(s); client must resync");
                serde_json::json!({ "kind": "lagged", "missed": n })
            }
            Err(RecvError::Closed) => break,
        };
        let frame = serde_json::json!({
            "type": "event",
            "event": "room:event",
            "payload": payload,
        });
        match serde_json::to_string(&frame) {
            Ok(text) => {
                if tx.send(Outbound::Text(text)).is_err() {
                    break; // client went away
                }
            }
            Err(e) => log::error!("serializing room event failed: {e}"),
        }
    }
}
