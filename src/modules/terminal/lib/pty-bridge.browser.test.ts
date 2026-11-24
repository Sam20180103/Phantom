import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { tokenBytes } from "./ptyWire";
import { decodeRequest, encodeFrames } from "./ptyWireFixtures";

const mocks = vi.hoisted(() => ({
  invoke: vi.fn(),
  open: vi.fn(),
  poll: vi.fn(),
  ack: vi.fn(),
  fetch: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: mocks.invoke,
  isTauri: () => false,
  convertFileSrc: () => { throw new Error("browser must not use native protocol"); },
}));
vi.mock("@tauri-apps/api/event", () => ({ listen: async () => () => {} }));
vi.mock("@/modules/workspace", () => ({ currentWorkspaceEnv: () => null }));
vi.mock("@/lib/transport/pty-stream", () => ({
  webSocketPtyTransport: () => ({ open: mocks.open, poll: mocks.poll, ack: mocks.ack }),
}));

describe("Ghostty browser bridge", () => {
  beforeEach(() => {
    vi.resetModules();
    vi.resetAllMocks();
    vi.useFakeTimers();
    vi.stubGlobal("fetch", mocks.fetch);
    mocks.open.mockResolvedValue(tokenBytes("00000000000000aa"));
    mocks.poll.mockImplementation(() => new Promise(() => {}));
    mocks.ack.mockResolvedValue({ status: 200, body: new Uint8Array() });
    mocks.invoke.mockImplementation(async (command: string) =>
      command === "pty_open" || command === "pty_attach" ? 41 : undefined,
    );
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("retains stable rmux identity and sends keyboard bytes without re-encoding", async () => {
    const { openPty } = await import("./pty-bridge");
    const session = await openPty(80, 24, { onData: vi.fn() }, "/repo", false, "bash", 7, "phantom-stable");
    expect(mocks.invoke).toHaveBeenCalledWith("pty_open", expect.objectContaining({
      session: "phantom-stable", paneId: 7, cols: 80, rows: 24,
    }));
    const bytes = Uint8Array.of(0, 255, 27, 13);
    await session.write(bytes);
    expect(mocks.invoke).toHaveBeenCalledWith("pty_write", bytes, { headers: { "x-pty-id": "41" } });
    await vi.advanceTimersByTimeAsync(0);
    expect(decodeRequest(mocks.poll.mock.calls[0][0]).acks).toEqual([{ id: 41, bytes: 0 }]);
    expect(mocks.fetch).not.toHaveBeenCalled();
    await session.close();
  });

  it("attaches an existing rmux session and cancels its poll exactly once on close", async () => {
    const { openPty } = await import("./pty-bridge");
    const session = await openPty(90, 30, { onData: vi.fn() }, undefined, false, undefined, 8, "phantom-existing", "phantom-existing");
    expect(mocks.invoke).toHaveBeenCalledWith("pty_attach", expect.objectContaining({ name: "phantom-existing", paneId: 8 }));
    expect(mocks.invoke.mock.calls.some(([command]) => command === "pty_open")).toBe(false);
    await vi.advanceTimersByTimeAsync(0);
    const signal = mocks.poll.mock.calls[0][1] as AbortSignal;
    await session.close();
    await session.close();
    expect(signal.aborted).toBe(true);
    expect(mocks.invoke.mock.calls.filter(([command]) => command === "pty_close")).toHaveLength(1);
  });

  it("returns credit only after the UI parser finishes", async () => {
    const { openPty } = await import("./pty-bridge");
    let deliver!: (value: { status: number; body: Uint8Array }) => void;
    mocks.poll.mockImplementation(() => new Promise(resolve => { deliver = resolve; }));
    let parsed!: () => void;
    const session = await openPty(80, 24, { onData: () => new Promise<void>(resolve => { parsed = resolve; }) });
    await vi.advanceTimersByTimeAsync(0);
    deliver({ status: 200, body: encodeFrames([{ kind: "data", id: 41, offset: 0, bytes: Uint8Array.of(1, 2, 3) }]) });
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.poll.mock.calls.every(([body]) => decodeRequest(body).acks.every(ack => ack.bytes === 0))).toBe(true);
    parsed();
    await vi.advanceTimersByTimeAsync(0);
    deliver({ status: 200, body: new Uint8Array() });
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.poll.mock.calls.some(([body]) => decodeRequest(body).acks.some(ack => ack.bytes === 3))).toBe(true);
    await session.close();
  });

  it("restores an attached terminal's cwd without waiting for a shell prompt", async () => {
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "pty_attach") return 41;
      if (command === "pty_get_cwd") return "/path/to/Phantom";
    });
    const { openPty } = await import("./pty-bridge");
    const onCwd = vi.fn();
    const session = await openPty(80, 24, { onData: vi.fn(), onCwd }, undefined, false, undefined, 7, undefined, "phantom-existing");
    await vi.advanceTimersByTimeAsync(0);
    expect(mocks.invoke).toHaveBeenCalledWith("pty_get_cwd", { id: 41 });
    expect(onCwd).toHaveBeenCalledWith("/path/to/Phantom");
    expect(mocks.invoke.mock.calls.some(([command]) => command === "pty_write")).toBe(false);
    await session.close();
  });

  it("ignores cwd snapshots arriving after the terminal closes", async () => {
    let resolve!: (cwd: string) => void;
    mocks.invoke.mockImplementation(async (command: string) => {
      if (command === "pty_attach") return 41;
      if (command === "pty_get_cwd") return new Promise<string>(done => { resolve = done; });
    });
    const { openPty } = await import("./pty-bridge");
    const onCwd = vi.fn();
    const session = await openPty(80, 24, { onData: vi.fn(), onCwd }, undefined, false, undefined, 7, undefined, "phantom-existing");
    await session.close();
    resolve("/old/session");
    await vi.advanceTimersByTimeAsync(0);
    expect(onCwd).not.toHaveBeenCalled();
  });
});
