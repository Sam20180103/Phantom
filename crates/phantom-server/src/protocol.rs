//! Wire protocol between the browser and `phantom-server`.
//!
//! Two framings share one socket, split by cost:
//!
//! * **Binary frames** carry terminal I/O — the per-keystroke and per-chunk hot
//!   path. Layout is `[u8 tag][u32 le pty id][payload…]`. This mirrors what
//!   Terax's Tauri IPC did with a raw body plus an `x-pty-id` header, and for
//!   the same reason: JSON-encoding every keystroke on both sides is pure
//!   overhead on the latency-critical path.
//! * **Text frames** carry JSON control messages — open, resize, close, exit.
//!   Low frequency, so legibility while debugging beats compactness.
//!
//! The request/response shape deliberately mirrors Tauri's `invoke` + `Channel`
//! model so the browser-side shim stays thin; see `src/lib/transport/`.

use serde::{Deserialize, Serialize};
use phantom_core::pty::stream::Endpoint;

/// Terminal bytes, either direction. `[TAG_PTY_DATA][u32 le id][bytes…]`
pub const TAG_PTY_DATA: u8 = 0x01;

pub fn decode_stream_request(frame: &[u8]) -> Option<(Endpoint, u32, &[u8])> {
    if frame.len() < 5 || frame.len() > 5 + 12 + 256 * 12 {
        return None;
    }
    let endpoint = match frame[0] {
        0x02 => Endpoint::Poll,
        0x03 => Endpoint::Ack,
        _ => return None,
    };
    Some((endpoint, u32::from_le_bytes(frame[1..5].try_into().ok()?), &frame[5..]))
}

pub fn encode_stream_response(req: u32, status: u16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(7 + body.len());
    frame.push(0x02);
    frame.extend_from_slice(&req.to_le_bytes());
    frame.extend_from_slice(&status.to_le_bytes());
    frame.extend_from_slice(body);
    frame
}

/// Frame a binary terminal payload. Shared by both directions so the layout
/// can only ever be defined once.
pub fn encode_pty_data(id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + bytes.len());
    out.push(TAG_PTY_DATA);
    out.extend_from_slice(&id.to_le_bytes());
    out.extend_from_slice(bytes);
    out
}

/// Returns `(pty_id, payload)`, or `None` if the frame is malformed or carries
/// an unknown tag. Callers must not treat a malformed frame as fatal — a
/// buggy or hostile client shouldn't be able to drop the whole connection.
pub fn decode_pty_data(frame: &[u8]) -> Option<(u32, &[u8])> {
    if frame.len() < 5 || frame[0] != TAG_PTY_DATA {
        return None;
    }
    let id = u32::from_le_bytes([frame[1], frame[2], frame[3], frame[4]]);
    Some((id, &frame[5..]))
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// Mirrors Tauri's `invoke`: a command name plus arguments, correlated by
    /// `req` so the reply can be matched to the caller's promise.
    Invoke {
        req: u64,
        cmd: String,
        #[serde(default)]
        args: serde_json::Value,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Reply to an [`ClientMsg::Invoke`]. Exactly one of `ok`/`err` is set.
    Result {
        req: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        ok: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        err: Option<String>,
    },
    /// A PTY's child exited. Terminal for that id.
    PtyExit { id: u32, code: i32 },
    /// An AI agent changed state, parsed from OSC 777 markers in the stream.
    AgentSignal { id: u32, kind: String, agent: Option<String> },
    /// One event on a streaming command's channel, keyed by the `req` that
    /// opened it. The frontend routes these to the `Channel` it passed in.
    Stream { req: u64, event: serde_json::Value },
}

impl ServerMsg {
    pub fn ok(req: u64, value: serde_json::Value) -> Self {
        Self::Result { req, ok: Some(value), err: None }
    }

    pub fn err(req: u64, message: impl Into<String>) -> Self {
        Self::Result { req, ok: None, err: Some(message.into()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_data_round_trips() {
        let framed = encode_pty_data(7, b"hello");
        assert_eq!(decode_pty_data(&framed), Some((7, &b"hello"[..])));
    }

    #[test]
    fn empty_payload_is_valid() {
        // A zero-length write is meaningless but well-formed; it must decode
        // rather than be mistaken for a truncated frame.
        let framed = encode_pty_data(1, b"");
        assert_eq!(decode_pty_data(&framed), Some((1, &b""[..])));
    }

    #[test]
    fn rejects_truncated_and_unknown_frames() {
        assert_eq!(decode_pty_data(&[]), None);
        assert_eq!(decode_pty_data(&[TAG_PTY_DATA, 1, 2, 3]), None);
        assert_eq!(decode_pty_data(&[0xFF, 1, 2, 3, 4]), None);
    }

    #[test]
    fn id_is_little_endian() {
        let framed = encode_pty_data(0x0403_0201, b"");
        assert_eq!(&framed[1..5], &[0x01, 0x02, 0x03, 0x04]);
    }
}
