/**
 * A WebSocket connection to a room on *another* machine.
 *
 * The browser a hub serves already reaches its own room through the app's
 * transport. This is for every other client — the desktop app above all — which
 * is served by nobody and has to say where the room is and who it is.
 *
 * It speaks the same request/response shapes the local socket does, so the
 * store above cannot tell the two apart; the differences are all here:
 *
 * - **Auth is the first frame.** A browser cannot set an `Authorization` header
 *   on a WebSocket handshake, and a token in the URL would land in history and
 *   in server logs. So the socket opens, sends `auth`, and waits for `ready`.
 * - **Its credential is room-only.** A client token reaches the room's three
 *   commands and nothing else — it is not the login password, which would also
 *   open a shell on the far machine.
 */

const RECONNECT_MIN_MS = 1_000;
const RECONNECT_MAX_MS = 30_000;
/** A hub that never answers `ready` is not a hub we can use. */
const AUTH_TIMEOUT_MS = 10_000;
/**
 * A request the hub never answers. A socket can look open long after the far
 * end is gone — a laptop that slept, a route that vanished — and without this
 * whatever waits on the reply (the Send button) waits forever.
 */
export const INVOKE_TIMEOUT_MS = 15_000;
/**
 * Silence after which a connection is taken for dead and replaced. A page can't
 * see pings, so the server sends a keepalive it can see; a socket that looks
 * open but hears nothing — a laptop that slept, a route that vanished — would
 * otherwise sit there quietly out of date.
 */
export const SILENCE_MS = 60_000;

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void };

/** Turns what a person types into the socket URL. */
export function roomSocketUrl(base: string): string {
  const trimmed = base.trim().replace(/\/+$/, "");
  const withScheme = /^[a-z]+:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`;
  return `${withScheme.replace(/^http/i, "ws")}/room/client`;
}

export class RoomClient {
  #url: string;
  #token: string;
  #socket: WebSocket | null = null;
  #pending = new Map<number, Pending>();
  #nextReq = 1;
  #events = new Set<(payload: unknown) => void>();
  #opens = new Set<() => void>();
  #closed = false;
  #backoff = RECONNECT_MIN_MS;
  /** Resolves on the first successful auth; rejects if that first try fails. */
  #firstReady: Promise<void>;

  constructor(base: string, token: string) {
    this.#url = roomSocketUrl(base);
    this.#token = token;
    let settle!: () => void;
    let fail!: (e: Error) => void;
    this.#firstReady = new Promise((res, rej) => {
      settle = res;
      fail = rej;
    });
    this.#connect(settle, fail);
  }

  /** Awaits the first successful connection. Later drops reconnect silently. */
  ready(): Promise<void> {
    return this.#firstReady;
  }

  /** Fires whenever a connection is (re)established, so callers can refill. */
  onOpen(handler: () => void): () => void {
    this.#opens.add(handler);
    return () => this.#opens.delete(handler);
  }

  onEvent(handler: (payload: unknown) => void): () => void {
    this.#events.add(handler);
    return () => this.#events.delete(handler);
  }

  invoke<T>(cmd: string, args: unknown = {}): Promise<T> {
    const socket = this.#socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      return Promise.reject(new Error("not connected to the room"));
    }
    return new Promise<T>((resolve, reject) => {
      const req = this.#nextReq++;
      const timer = setTimeout(() => {
        this.#pending.delete(req);
        reject(new Error("the room did not answer"));
      }, INVOKE_TIMEOUT_MS);
      this.#pending.set(req, {
        resolve: (v) => {
          clearTimeout(timer);
          resolve(v as T);
        },
        reject: (e) => {
          clearTimeout(timer);
          reject(e);
        },
      });
      socket.send(JSON.stringify({ type: "invoke", req, cmd, args }));
    });
  }

  close(): void {
    this.#closed = true;
    this.#socket?.close();
    this.#socket = null;
    this.#failPending(new Error("room connection closed"));
  }

  #connect(settle: () => void, fail: (e: Error) => void): void {
    if (this.#closed) return;

    let socket: WebSocket;
    try {
      socket = new WebSocket(this.#url);
    } catch (e) {
      fail(e as Error);
      return;
    }

    // A hub that accepts the socket but never answers `ready` — wrong token,
    // or not a Phantom server at all — would otherwise hang forever.
    const authTimer = setTimeout(() => socket.close(), AUTH_TIMEOUT_MS);
    let authed = false;
    // Only once a keepalive has been seen: a hub too old to send them would
    // otherwise look dead every quiet minute.
    let heard = Date.now();
    let keepsAlive = false;
    const watchdog = setInterval(() => {
      if (keepsAlive && Date.now() - heard > SILENCE_MS) socket.close();
    }, SILENCE_MS / 6);

    socket.onopen = () => {
      socket.send(JSON.stringify({ type: "auth", token: this.#token }));
    };

    socket.onmessage = (event) => {
      heard = Date.now();
      if (typeof event.data !== "string") return;
      let msg: { type?: string; req?: number; ok?: unknown; err?: string; payload?: unknown };
      try {
        msg = JSON.parse(event.data);
      } catch {
        return;
      }

      if (msg.type === "keepalive") {
        keepsAlive = true;
        return;
      }
      if (msg.type === "ready") {
        clearTimeout(authTimer);
        authed = true;
        this.#socket = socket;
        this.#backoff = RECONNECT_MIN_MS;
        settle();
        for (const h of this.#opens) h();
        return;
      }
      if (msg.type === "result" && msg.req !== undefined) {
        const p = this.#pending.get(msg.req);
        this.#pending.delete(msg.req);
        if (msg.err !== undefined) p?.reject(new Error(msg.err));
        else p?.resolve(msg.ok);
        return;
      }
      if (msg.type === "event") {
        for (const h of this.#events) h(msg.payload);
      }
    };

    socket.onerror = () => {
      // `onclose` always follows and does the teardown.
    };

    socket.onclose = () => {
      clearTimeout(authTimer);
      clearInterval(watchdog);
      this.#socket = null;
      this.#failPending(new Error("room connection lost"));
      if (this.#closed) return;
      if (!authed) {
        // The very first attempt failed: tell the caller, rather than
        // retrying forever behind a promise nobody will ever see settle.
        fail(new Error(`could not join the room at ${this.#url}`));
      }
      setTimeout(() => this.#connect(settle, fail), this.#backoff);
      this.#backoff = Math.min(this.#backoff * 2, RECONNECT_MAX_MS);
    };
  }

  #failPending(error: Error): void {
    for (const p of this.#pending.values()) p.reject(error);
    this.#pending.clear();
  }
}
