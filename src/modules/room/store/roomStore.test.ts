import { describe, expect, it, vi } from "vitest";
import type { RoomMsg } from "../lib/room";

const { postMessage, history } = vi.hoisted(() => ({
  postMessage: vi.fn(),
  history: vi.fn(),
}));

vi.mock("../lib/room", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../lib/room")>()),
  postMessage,
  history,
}));

import { MAX_MESSAGES, mergeMessages, useRoomStore } from "./roomStore";

const msg = (seq: number, text = `m${seq}`): RoomMsg => ({
  seq,
  ts: 0,
  from: { peer: "local", agent: null },
  mentions: [],
  text,
  refs: [],
});

describe("mergeMessages", () => {
  it("dedupes a message that arrives twice", () => {
    // A post is seen twice by design: once as the invoke's reply, once as the
    // broadcast echo. Rendering it twice would be the obvious bug.
    expect(mergeMessages([msg(1)], [msg(1)]).length).toBe(1);
  });

  it("orders by seq regardless of arrival order", () => {
    const merged = mergeMessages([msg(3)], [msg(1), msg(2)]);
    expect(merged.map((m) => m.seq)).toEqual([1, 2, 3]);
  });

  it("lets a refill overwrite what is already held", () => {
    const merged = mergeMessages([msg(1, "stale")], [msg(1, "fresh")]);
    expect(merged[0].text).toBe("fresh");
  });

  it("keeps the newest messages when over the cap", () => {
    const many = Array.from({ length: MAX_MESSAGES + 10 }, (_, i) => msg(i));
    const merged = mergeMessages([], many);
    expect(merged.length).toBe(MAX_MESSAGES);
    expect(merged[merged.length - 1].seq).toBe(MAX_MESSAGES + 9);
  });
});

describe("send", () => {
  it("rejects a refused post without blanking the room", async () => {
    // The panel keeps the draft only if `send` rejects, and a refusal (a peer
    // whose hub is briefly down) says nothing about whether the room exists.
    postMessage.mockRejectedValueOnce(
      new Error("not connected to the room's hub"),
    );
    await expect(useRoomStore.getState().send("hi")).rejects.toThrow("hub");
    expect(useRoomStore.getState().unavailable).toBeNull();
  });

  it("shows an accepted post without waiting for its echo", async () => {
    postMessage.mockResolvedValueOnce(msg(7, "hi"));
    await useRoomStore.getState().send("hi");
    expect(useRoomStore.getState().messages.map((m) => m.seq)).toContain(7);
  });
});

describe("refill", () => {
  const page = (epoch: string, messages: RoomMsg[]) => ({
    room: "main",
    localPeer: "local",
    epoch,
    messages,
  });

  it("merges what is new on the same log", async () => {
    useRoomStore.setState({ messages: [msg(1)], epoch: "e1" });
    history.mockResolvedValueOnce(page("e1", [msg(2)]));
    await useRoomStore.getState().refill();
    expect(useRoomStore.getState().messages.map((m) => m.seq)).toEqual([1, 2]);
  });

  it("starts over when the log it held was replaced", async () => {
    // Held: 40 and 41 from the old log. The new log's 0 must not be merged
    // among them, and "after 41" means nothing in its numbering.
    useRoomStore.setState({
      messages: [msg(40, "old"), msg(41, "old")],
      epoch: "e1",
    });
    history
      .mockResolvedValueOnce(page("e2", []))
      .mockResolvedValueOnce(page("e2", [msg(0, "new")]));
    await useRoomStore.getState().refill();
    const s = useRoomStore.getState();
    expect(s.messages.map((m) => m.text)).toEqual(["new"]);
    expect(s.epoch).toBe("e2");
  });
});
