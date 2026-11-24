import { flushPersist, useChatStore } from "@/modules/ai/store/chatStore";
import type { UIMessage } from "@ai-sdk/react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";

const sessions = vi.hoisted(() => ({
  saveMessages: vi.fn(async (_id: string, _messages: UIMessage[]) => {}),
  loadAll: vi.fn(),
  loadMessages: vi.fn(),
  saveSessionsList: vi.fn(),
  saveActiveId: vi.fn(),
  deleteSessionData: vi.fn(),
  deriveTitle: vi.fn(() => "New chat"),
  newSessionId: vi.fn(),
}));
vi.mock("@/modules/ai/lib/sessions", () => sessions);

beforeEach(() => vi.clearAllMocks());

afterEach(() => {
  flushPersist();
  vi.useRealTimers();
});

it("saves the latest messages during continuous streaming", async () => {
  vi.useFakeTimers();
  const persist = useChatStore.getState().persistMessages;
  for (let elapsed = 0; elapsed < 5_000; elapsed += 100) {
    persist("stream", [{ id: String(elapsed), role: "assistant", parts: [] }]);
    await vi.advanceTimersByTimeAsync(100);
  }
  expect(sessions.saveMessages).toHaveBeenCalled();
  expect(sessions.saveMessages.mock.calls[0][1][0].id).toBe("3900");
  expect(sessions.saveMessages).toHaveBeenCalledWith(
    "stream",
    expect.any(Array),
    { durable: false },
  );
});

it("flushes the latest snapshot durably and cancels its timer", async () => {
  vi.useFakeTimers();
  const messages: UIMessage[] = [{ id: "last", role: "assistant", parts: [] }];
  useChatStore.getState().persistMessages("idle", messages);
  flushPersist("idle");
  expect(sessions.saveMessages).toHaveBeenCalledWith("idle", messages, {
    durable: true,
  });
  await vi.advanceTimersByTimeAsync(5_000);
  expect(sessions.saveMessages).toHaveBeenCalledOnce();
});
