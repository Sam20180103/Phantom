/**
 * The single WebSocket connection to `phantom-server`.
 *
 * This is the seam that replaces Tauri IPC. Everything above it — the whole
 * Terax frontend — keeps calling `invoke()` and `Channel` exactly as before;
 * a Vite alias points `@tauri-apps/api/core` at our shim instead. That is why
 * this file mimics Tauri's request/response and channel semantics rather than
 * inventing a nicer API.
 *
 * Scope for now: terminal I/O only. Reconnection is deliberately naive — real
 * session resync (sequence numbers, incremental replay) belongs with the rmux
 * work, see the implementation plan §6.2.
 */

import { decodePtyData, encodePtyData, type ServerMsg } from "./protocol";
import {
  decodePtyStreamReply,
  encodePtyStreamRequest,
  type PtyStreamEndpoint,
  type PtyStreamReply,
} from "@/lib/transport/protocol";

/** Mirrors `@tauri-apps/api/core`'s `Channel`: a server-push stream. */
export class Channel<T = unknown> {
  onmessage: (message: T) => void = () => {};
}

type Pending = {
  resolve: (value: unknown) => void;
  reject: (reason: Error) => void;
};

/** The channel pair a `pty_open` call registered, keyed by the PTY id. */
type PtyChannels = {
  onData?: Channel<ArrayBuffer>;
  onExit?: Channel<number>;
};

const RECONNECT_DELAY_MS = 1000;

function socketUrl(): string {
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}/ws`;
}

class Transport {
  #ready: Promise<WebSocket> | null = null;
  #nextReq = 1;
  #pending = new Map<number, Pending>();
  #ptyStreamRequests = new Map<
    number,
    {
      resolve: (reply: PtyStreamReply) => void;
      reject: (error: Error) => void;
    }
  >();
  #ptyChannels = new Map<number, PtyChannels>();
  // A streaming command's `onEvent` channel, keyed by the request id rather
  // than a server-allocated id: unlike a PTY there's no handle to key on, and
  // the request is 1:1 with the stream. Dropped when the result arrives.
  #streamChannels = new Map<number, Channel<unknown>>();
  #listeners = new Map<string, Set<(payload: unknown) => void>>();

  #connect(): Promise<WebSocket> {
    if (this.#ready) return this.#ready;

    this.#ready = new Promise((resolve, reject) => {
      const socket = new WebSocket(socketUrl());
      socket.binaryType = "arraybuffer";

      socket.onopen = () => {
        resolve(socket);
        // Server-pushed feeds (the room) are backed by a task that dies with
        // the old socket and cannot replay what it sent while we were gone.
        // Announce the new connection so those listeners refill themselves.
        this.dispatch("phantom:connected", undefined);
      };

      socket.onmessage = (event) => this.#receive(event.data);

      socket.onerror = () => {
        // `onclose` always follows, and does the teardown. Rejecting here
        // covers the case where we never opened at all.
        reject(new Error("phantom: WebSocket error"));
      };

      socket.onclose = () => {
        this.#ready = null;
        // Fail in-flight calls rather than leaving their promises dangling
        // forever — callers surface this as an error instead of hanging.
        const disconnected = new Error("phantom: disconnected");
        for (const pending of this.#pending.values())
          pending.reject(disconnected);
        this.#pending.clear();
        for (const pending of this.#ptyStreamRequests.values())
          pending.reject(disconnected);
        // These channels are bound to pty ids from the old socket, which mean
        // nothing to the new one, so drop them rather than leaking closures.
        // Under the rmux backend the shells themselves keep running and are
        // recovered through `pty_sessions` / `pty_attach`, not through here.
        this.#ptyChannels.clear();
        this.#streamChannels.clear();
        setTimeout(
          () => void this.#connect().catch(() => {}),
          RECONNECT_DELAY_MS,
        );
      };
    });

    return this.#ready;
  }

  #receive(data: unknown): void {
    if (data instanceof ArrayBuffer) {
      if (data.byteLength >= 10 && new DataView(data).getUint8(0) === 0x04) {
        const req = Number(new DataView(data).getBigUint64(1, true));
        this.#streamChannels.get(req)?.onmessage(data.slice(9));
        return;
      }
      const reply = decodePtyStreamReply(data);
      if (reply) {
        this.#ptyStreamRequests.get(reply.req)?.resolve(reply);
        return;
      }
      const frame = decodePtyData(data);
      if (!frame) return;
      this.#ptyChannels.get(frame.id)?.onData?.onmessage(frame.payload);
      return;
    }

    if (typeof data !== "string") return;

    let msg: ServerMsg;
    try {
      msg = JSON.parse(data) as ServerMsg;
    } catch {
      return;
    }

    switch (msg.type) {
      case "result": {
        const pending = this.#pending.get(msg.req);
        if (!pending) return;
        this.#pending.delete(msg.req);
        if (msg.err !== undefined) pending.reject(new Error(msg.err));
        else pending.resolve(msg.ok ?? null);
        return;
      }
      case "pty_exit": {
        const channels = this.#ptyChannels.get(msg.id);
        this.#ptyChannels.delete(msg.id);
        channels?.onExit?.onmessage(msg.code);
        return;
      }
      case "agent_signal": {
        // Terax's frontend listens for this via the event API.
        this.dispatch("terax:agent-signal", {
          id: msg.id,
          kind: msg.kind,
          agent: msg.agent,
        });
        return;
      }
      case "event": {
        // Generic server-originated event, named exactly as the desktop build
        // emits it (`fs:changed`, `fs:file-written`, …) so the `listen()` call
        // sites need no change.
        this.dispatch(msg.event, msg.payload);
        return;
      }
      case "stream": {
        // Mirrors Tauri's Channel: hand the event to the registered onmessage.
        this.#streamChannels.get(msg.req)?.onmessage(msg.event);
        return;
      }
    }
  }

  /** Deliver to listeners in this page, whatever the event's origin. */
  dispatch(name: string, payload: unknown): void {
    for (const handler of this.#listeners.get(name) ?? []) handler(payload);
  }

  listen(name: string, handler: (payload: unknown) => void): () => void {
    let handlers = this.#listeners.get(name);
    if (!handlers) {
      handlers = new Set();
      this.#listeners.set(name, handlers);
    }
    handlers.add(handler);
    return () => {
      handlers.delete(handler);
    };
  }

  /**
   * Terminal input. Sent as a binary frame rather than an `invoke` because
   * this runs once per keystroke; see `protocol.rs` for why.
   */
  async ptyWrite(id: number, bytes: Uint8Array): Promise<void> {
    const socket = await this.#connect();
    socket.send(encodePtyData(id, bytes));
  }

  ptyStreamRequest(
    endpoint: PtyStreamEndpoint,
    body: Uint8Array,
    signal?: AbortSignal,
  ): Promise<PtyStreamReply> {
    const req = this.#nextReq++;
    return new Promise((resolve, reject) => {
      const cleanup = () => {
        this.#ptyStreamRequests.delete(req);
        signal?.removeEventListener("abort", abort);
      };
      const fail = (error: Error) => {
        cleanup();
        reject(error);
      };
      const abort = () =>
        fail(new DOMException("PTY stream request aborted", "AbortError"));
      if (signal?.aborted) {
        abort();
        return;
      }
      signal?.addEventListener("abort", abort, { once: true });
      this.#ptyStreamRequests.set(req, {
        resolve: (reply) => {
          cleanup();
          resolve(reply);
        },
        reject: fail,
      });
      void this.#connect().then((socket) => {
        if (!this.#ptyStreamRequests.has(req)) return;
        try {
          socket.send(encodePtyStreamRequest(req, endpoint, body));
        } catch (error) {
          fail(error instanceof Error ? error : new Error(String(error)));
        }
      }, fail);
    });
  }

  async invoke<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
    // Channels are objects, not JSON — pull them out and bind them to the PTY
    // id the server hands back. Only the two PTY-opening commands use channels
    // today; a general channel-registration protocol can wait until something
    // else needs one.
    let channels: PtyChannels | null = null;
    let streamChannel: Channel<unknown> | null = null;
    let payload = args;
    if (cmd === "pty_open" || cmd === "pty_attach") {
      const { onData, onExit, ...rest } = args as {
        onData?: Channel<ArrayBuffer>;
        onExit?: Channel<number>;
      };
      if (onData || onExit) channels = { onData, onExit };
      payload = rest;
    } else if (cmd === "ai_http_stream" || cmd === "orchestrate") {
      const { onEvent, ...rest } = args as { onEvent?: Channel<unknown> };
      streamChannel = onEvent ?? null;
      payload = rest;
    }

    const socket = await this.#connect();
    const req = this.#nextReq++;
    // Register the stream channel before sending: events can arrive before the
    // final result, and would be dropped if registered after the await.
    if (streamChannel) this.#streamChannels.set(req, streamChannel);
    const result = new Promise<T>((resolve, reject) => {
      this.#pending.set(req, {
        resolve: resolve as (v: unknown) => void,
        reject,
      });
    });

    socket.send(JSON.stringify({ type: "invoke", req, cmd, args: payload }));

    try {
      const value = await result;
      if (channels) this.#ptyChannels.set(value as number, channels);
      return value;
    } finally {
      // The stream ends with the result (ok) or an error; either way it is done.
      this.#streamChannels.delete(req);
    }
  }
}

export const transport = new Transport();
