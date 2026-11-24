import { create } from "zustand";
import type { UnlistenFn } from "@tauri-apps/api/event";
import { getRoomToken } from "../lib/credential";
import { usePreferencesStore } from "@/modules/settings/preferences";
import {
  connectRoom,
  history,
  onRoomConnected,
  onRoomEvent,
  postMessage,
  roster as fetchRoster,
  type Attachment,
  type RoomMsg,
  type RoomRef,
  type RosterEntry,
} from "../lib/room";

/**
 * Room state for the whole page.
 *
 * A store rather than component state because the feed is server-pushed and has
 * to survive the panel being closed — messages keep arriving whether or not
 * anyone is looking, and reopening must not lose them or re-fetch the world.
 */

export const MAX_MESSAGES = 500;

/**
 * The feed listeners `subscribe` registered. `reconnect` drops them before
 * subscribing again; otherwise every "Save and connect" would stack one more
 * listener on the same event, and each would refill on every reconnect.
 */
const unlisteners: UnlistenFn[] = [];

type RoomStoreState = {
  messages: RoomMsg[];
  roster: RosterEntry[];
  localPeer: string;
  room: string;
  /** The hub this room lives on, or null when it is this server's own. */
  hub: string | null;
  /** Set when the server told us we fell behind; cleared by the refill. */
  gap: boolean;
  /** The log the held messages belong to; see `history`. */
  epoch: string | null;
  /**
   * Why the room is unavailable, if it is. A backend without the room commands
   * — the desktop app, or an older server — answers "unknown command" rather
   * than a message list, and that has to read as an empty panel, not a crash.
   */
  unavailable: string | null;
  subscribed: boolean;
  /** Idempotent: safe to call from every mount. */
  subscribe: () => Promise<void>;
  /** Pulls whatever arrived after the newest message we hold. */
  refill: () => Promise<void>;
  /** Re-reads the hub settings and reconnects. Call after changing them. */
  reconnect: () => Promise<void>;
  refreshRoster: (cwd: string) => Promise<void>;
  send: (
    text: string,
    refs?: RoomRef[],
    attach?: Attachment[],
  ) => Promise<void>;
};

/**
 * Inserts by `seq`, dropping duplicates.
 *
 * Both a post's own reply and its broadcast echo carry the same message, and a
 * refill after a gap overlaps what already arrived — so ordering and dedupe
 * live here rather than being assumed at each call site.
 */
export function mergeMessages(existing: RoomMsg[], incoming: RoomMsg[]): RoomMsg[] {
  const bySeq = new Map(existing.map((m) => [m.seq, m]));
  for (const m of incoming) bySeq.set(m.seq, m);
  const all = [...bySeq.values()].sort((a, b) => a.seq - b.seq);
  return all.length > MAX_MESSAGES ? all.slice(-MAX_MESSAGES) : all;
}

export const useRoomStore = create<RoomStoreState>((set, get) => ({
  messages: [],
  roster: [],
  localPeer: "local",
  room: "",
  gap: false,
  epoch: null,
  hub: null,
  unavailable: null,
  subscribed: false,

  subscribe: async () => {
    if (get().subscribed) return;
    set({ subscribed: true });

    // A configured hub means the room is on another machine; without one the
    // server that served this page is the room. Failing to reach a configured
    // hub is reported, not silently downgraded to the local one — that would
    // quietly put messages in the wrong room.
    // Preferences may not be loaded yet when the panel first mounts. Reading
    // early would see the empty default, silently fall back to this machine's
    // room, and put messages somewhere the user never chose. `init` is
    // idempotent, so awaiting it here costs nothing after the first call.
    await usePreferencesStore.getState().init();
    const hubUrl = usePreferencesStore.getState().roomHubUrl.trim();
    const token = await getRoomToken();
    if (hubUrl) {
      try {
        await connectRoom({ url: hubUrl, token: token ?? "" });
        set({ hub: hubUrl });
      } catch (e) {
        // `hub` too: the room lives there, and the panel should say that it
        // couldn't be reached rather than that there is no room at all.
        set({
          unavailable: (e as Error).message,
          subscribed: false,
          hub: hubUrl,
        });
        return;
      }
    }

    unlisteners.push(
      await onRoomEvent((e) => {
        if (e.kind === "message") {
          // From a log other than the one held: the refill starts over.
          const held = get().epoch;
          if (e.epoch && held && e.epoch !== held) {
            void get().refill();
            return;
          }
          set((s) => ({ messages: mergeMessages(s.messages, [e.message]) }));
          return;
        }
        // Fell behind the server's broadcast buffer. Refill from the last seq
        // we actually hold rather than guessing at what was missed.
        set({ gap: true });
        void get().refill();
      }),
    );

    // A reconnect gets a fresh server-side feed task, which knows nothing about
    // what it missed while the socket was down. Refill on every connect; the
    // first one is the initial load.
    unlisteners.push(await onRoomConnected(() => void get().refill()));
    await get().refill();
  },

  refill: async () => {
    const held = get().messages;
    try {
      const h = await history(held[held.length - 1]?.seq);
      const known = get().epoch;
      if (h.epoch && known && h.epoch !== known) {
        // The log was replaced. What is held is numbered in the old one's
        // terms, so merging would interleave two histories: start over.
        const fresh = await history();
        set({
          messages: mergeMessages([], fresh.messages),
          epoch: fresh.epoch ?? null,
          localPeer: fresh.localPeer,
          room: fresh.room,
          gap: false,
          unavailable: null,
        });
        return;
      }
      set((s) => ({
        messages: mergeMessages(s.messages, h.messages),
        epoch: h.epoch ?? s.epoch,
        localPeer: h.localPeer,
        room: h.room,
        gap: false,
        unavailable: null,
      }));
    } catch (e) {
      set({ unavailable: (e as Error).message ?? String(e), gap: false });
    }
  },

  reconnect: async () => {
    for (const off of unlisteners.splice(0)) off();
    await connectRoom(null);
    set({
      subscribed: false,
      messages: [],
      roster: [],
      hub: null,
      unavailable: null,
      epoch: null,
    });
    await get().subscribe();
  },

  refreshRoster: async (cwd) => set({ roster: await fetchRoster(cwd) }),

  // Rejects when the post did not land, so the caller can keep the draft. A
  // refused post says nothing about whether the room exists, so it must not
  // touch `unavailable` — that would blank the whole conversation.
  send: async (text, refs = [], attach = []) => {
    const msg = await postMessage(text, refs, attach);
    // Null on a peer server, where the hub sequences the message and it arrives
    // on the feed instead. Showing it optimistically would give it a fake seq.
    if (msg) set((s) => ({ messages: mergeMessages(s.messages, [msg]) }));
  },
}));
