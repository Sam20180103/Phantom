import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { currentWorkspaceEnv } from "@/modules/workspace";
import { RoomClient } from "./roomClient";

/**
 * The room: one shared message log that the person at the browser and the CLI
 * agents both post into.
 *
 * The point is cross-machine work. Local agents already collaborate through
 * `.phantom/handoff/*.md` (see `Phantom-任务清单.md` §二 P1), which only works
 * while they share a filesystem. The room carries *who needs what* — the
 * content itself still travels as a reference to a file or a blob.
 */

/** `agent === null` means a human at that server. */
export type Participant = {
  peer: string;
  agent: string | null;
};

/**
 * A pointer to content that lives outside the message. These two are the whole
 * surface a remote participant can read — there is no arbitrary-path variant.
 */
export type RoomRef =
  | { kind: "handoff"; peer: string; file: string }
  | { kind: "blob"; peer: string; id: string };

export type RoomMsg = {
  /** Server-assigned, monotonic. Used to dedupe and to detect gaps. */
  seq: number;
  ts: number;
  from: Participant;
  mentions: Participant[];
  text: string;
  refs: RoomRef[];
  /**
   * `notice` is the room reporting on itself — a delivery that failed, a peer
   * that isn't connected — not anyone speaking. Absent means an ordinary message.
   */
  kind?: "say" | "notice";
};

/** One mentionable participant, as the roster reports it. */
export type RosterEntry = {
  /** `peer/agent`, or `peer/human`. Stable identity, used in `@` completion. */
  id: string;
  peer: string;
  agent: string | null;
  role: string | null;
  running: boolean;
  /**
   * What the agent's hooks last reported: `working`/`started`, `attention`,
   * `finished`, or `exited` (its CLI quit; messages to it are refused). Absent
   * before the first hook fires, and from older servers.
   */
  state?: string | null;
};

/** Text to keep as a file on the server this client talks to, and reference. */
export type Attachment = { name: string; text: string };

/** The server's own limits, so a post is shaped to fit before it is sent. */
export const MAX_MESSAGE_BYTES = 8000;
export const MAX_ATTACHMENT_BYTES = 64 * 1024;
export const MAX_ATTACHMENTS = 4;

/**
 * What actually goes out: the message, and anything too long to be one as a
 * file. The opening line stays the message — that is where an `@mention`
 * usually is, and a mention only routes from the message itself. `null` when
 * there is nothing to send.
 */
export function composePost(
  text: string,
  attached: Attachment[],
): { text: string; attach: Attachment[] } | null {
  let body = text.trim();
  const attach = [...attached];
  if (new TextEncoder().encode(body).length > MAX_MESSAGE_BYTES) {
    attach.push({ name: "message.txt", text: body });
    const first = body.split("\n")[0];
    body = `${first.length <= 400 ? first : body.slice(0, 200)} … (full text attached)`;
  }
  if (!body && attach.length)
    body = `📎 ${attach.map((a) => a.name).join(", ")}`;
  return body ? { text: body, attach } : null;
}

/** Server push: either a message, or a notice that this client fell behind. */
export type RoomEvent =
  | {
      kind: "message";
      message: RoomMsg;
      /** The log it belongs to; see `history`. */
      epoch?: string;
    }
  | { kind: "lagged"; missed: number };

/**
 * The connection everything below goes through.
 *
 * `null` means "the server that served this page is the room" — the browser a
 * hub serves, where the app's own transport already reaches it. Set it and the
 * room lives on another machine instead; nothing above this line changes.
 */
let remote: RoomClient | null = null;

/** Points the room at another machine, or back at this one when `hub` is null. */
export async function connectRoom(
  hub: { url: string; token: string } | null,
): Promise<void> {
  remote?.close();
  remote = null;
  if (!hub) return;
  const client = new RoomClient(hub.url, hub.token);
  try {
    await client.ready();
  } catch (e) {
    client.close();
    throw e;
  }
  remote = client;
}

export function isRemote(): boolean {
  return remote !== null;
}

function call<T>(cmd: string, args: Record<string, unknown>): Promise<T> {
  return remote ? remote.invoke<T>(cmd, args) : invoke<T>(cmd, args);
}

export function participantId(p: Participant): string {
  return `${p.peer}/${p.agent ?? "human"}`;
}

/**
 * Posts as the human at this server. Mentions are parsed from the text.
 *
 * Resolves to `null` when this server is a peer in someone else's room: the hub
 * assigns the sequence there, so the authoritative copy arrives moments later
 * on the feed rather than being echoed back here.
 */
export async function postMessage(
  text: string,
  refs: RoomRef[] = [],
  attach: Attachment[] = [],
): Promise<RoomMsg | null> {
  return call<RoomMsg | null>("room_post", { text, refs, attach });
}

/**
 * Messages after `since`, or the whole retained window when omitted. This is
 * also the gap repair: after a reconnect or a `lagged` notice, ask again from
 * the last seq that actually arrived.
 */
export async function history(since?: number): Promise<{
  room: string;
  localPeer: string;
  /**
   * Which log the numbers belong to. They start again when a log does — a
   * renamed room, a deleted file, a hub moved to another box — so a different
   * epoch means what is held came from another log. Absent from older servers.
   */
  epoch?: string;
  messages: RoomMsg[];
}> {
  return call("room_history", { since });
}

/**
 * What an agent's terminal shows right now, read-only. `target` is `agent` or
 * `peer/agent`. Refused by a machine that doesn't show its agents' screens.
 */
export async function peek(target: string): Promise<string> {
  return call<string>("room_peek", { target });
}

/** Who can be mentioned: this server's human plus its configured agents. */
export async function roster(cwd: string): Promise<RosterEntry[]> {
  try {
    // A remote room resolves its own workspace from the hub's config; only the
    // local case can say which folder this window has open.
    const args = remote ? {} : { cwd, workspace: currentWorkspaceEnv() };
    return await call<RosterEntry[]>("room_roster", args);
  } catch {
    // No agents.yaml, or a backend without rmux — nothing to address.
    return [];
  }
}

export function onRoomEvent(handler: (e: RoomEvent) => void): Promise<UnlistenFn> {
  if (remote) {
    return Promise.resolve(remote.onEvent((p) => handler(p as RoomEvent)));
  }
  return listen<RoomEvent>("room:event", (e) => handler(e.payload));
}

/**
 * Fires when a connection is (re)established and the client may have missed
 * messages. Both transports lose a reconnect's worth of history, so both need
 * to say so — the local one through the transport's own announcement.
 */
export function onRoomConnected(handler: () => void): Promise<UnlistenFn> {
  if (remote) return Promise.resolve(remote.onOpen(handler));
  return listen("phantom:connected", () => handler());
}
