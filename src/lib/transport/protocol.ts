/**
 * Browser half of the wire protocol. Keep in lockstep with
 * `crates/phantom-server/src/protocol.rs` — the two files are one contract,
 * and a change to either without the other is a silent break.
 */

/** Terminal bytes, either direction: `[TAG_PTY_DATA][u32 le id][bytes…]` */
const TAG_PTY_DATA = 0x01;

export type PtyStreamEndpoint = "poll" | "ack";
export type PtyStreamReply = {
  req: number;
  status: number;
  body: Uint8Array<ArrayBuffer>;
};

export function encodePtyStreamRequest(
  req: number,
  endpoint: PtyStreamEndpoint,
  body: Uint8Array,
): ArrayBuffer {
  if (
    !Number.isInteger(req) ||
    req < 1 ||
    req > 0xffffffff ||
    body.byteLength > 12 + 256 * 12
  ) {
    throw new Error("Invalid PTY stream request");
  }
  const frame = new Uint8Array(5 + body.byteLength);
  frame[0] = endpoint === "poll" ? 0x02 : 0x03;
  new DataView(frame.buffer).setUint32(1, req, true);
  frame.set(body, 5);
  return frame.buffer;
}

export function decodePtyStreamReply(
  frame: ArrayBuffer,
): PtyStreamReply | null {
  if (frame.byteLength < 7 || frame.byteLength > 7 + 1024 * 1024) return null;
  const view = new DataView(frame);
  if (view.getUint8(0) !== 0x02) return null;
  const status = view.getUint16(5, true);
  if (status < 100 || status > 599) return null;
  return {
    req: view.getUint32(1, true),
    status,
    body: new Uint8Array(frame, 7),
  };
}

export function encodePtyData(id: number, bytes: Uint8Array): ArrayBuffer {
  const frame = new Uint8Array(5 + bytes.byteLength);
  frame[0] = TAG_PTY_DATA;
  new DataView(frame.buffer).setUint32(1, id, /* littleEndian */ true);
  frame.set(bytes, 5);
  return frame.buffer;
}

export type PtyFrame = { id: number; payload: ArrayBuffer };

/** Returns `null` for a malformed or unknown frame; callers should ignore it. */
export function decodePtyData(frame: ArrayBuffer): PtyFrame | null {
  if (frame.byteLength < 5) return null;
  const view = new DataView(frame);
  if (view.getUint8(0) !== TAG_PTY_DATA) return null;
  return {
    id: view.getUint32(1, /* littleEndian */ true),
    payload: frame.slice(5),
  };
}

export type ServerMsg =
  | { type: "result"; req: number; ok?: unknown; err?: string }
  | { type: "pty_exit"; id: number; code: number }
  | { type: "agent_signal"; id: number; kind: string; agent: string | null }
  | { type: "event"; event: string; payload: unknown }
  | { type: "stream"; req: number; event: unknown };
