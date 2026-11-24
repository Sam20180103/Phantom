import { afterEach, describe, expect, it, vi } from "vitest";
import {
  INVOKE_TIMEOUT_MS,
  RoomClient,
  roomSocketUrl,
  SILENCE_MS,
} from "./roomClient";

describe("roomSocketUrl", () => {
  it("upgrades both schemes to their websocket form", () => {
    expect(roomSocketUrl("https://h:8099")).toBe("wss://h:8099/room/client");
    expect(roomSocketUrl("http://h:8099")).toBe("ws://h:8099/room/client");
  });

  it("assumes TLS when the scheme is left off", () => {
    // Typing a bare host is the common case, and defaulting to plaintext would
    // send a token in the clear across the mesh.
    expect(roomSocketUrl("192.0.2.5:8099")).toBe("wss://192.0.2.5:8099/room/client");
  });

  it("tolerates trailing slashes and padding", () => {
    expect(roomSocketUrl("  https://h:8099///  ")).toBe("wss://h:8099/room/client");
  });
})

/** Just enough of a WebSocket for the client to open, authenticate and send. */
class FakeSocket {
  static OPEN = 1;
  static last: FakeSocket;
  readyState = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((e: { data: string }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  constructor(public url: string) {
    FakeSocket.last = this;
  }
  send(data: string) {
    this.sent.push(data);
  }
  closed = false;
  close() {
    this.closed = true;
    this.onclose?.();
  }
}

describe("RoomClient.invoke", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("gives up on a request the hub never answers", async () => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new RoomClient("https://h:8099", "tok");
    const socket = FakeSocket.last;
    socket.readyState = FakeSocket.OPEN;
    socket.onopen?.();
    socket.onmessage?.({ data: JSON.stringify({ type: "ready" }) });
    await client.ready();

    // A half-open socket: the request goes out and nothing ever comes back.
    const reply = client.invoke("room_post", { text: "hi" });
    vi.advanceTimersByTime(INVOKE_TIMEOUT_MS);
    await expect(reply).rejects.toThrow("did not answer");
    client.close();
  });
});

describe("RoomClient liveness", () => {
  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const joined = async () => {
    vi.useFakeTimers();
    vi.stubGlobal("WebSocket", FakeSocket);
    const client = new RoomClient("https://h:8099", "tok");
    const socket = FakeSocket.last;
    socket.readyState = FakeSocket.OPEN;
    socket.onopen?.();
    socket.onmessage?.({ data: JSON.stringify({ type: "ready" }) });
    await client.ready();
    return { client, socket };
  };

  it("replaces a connection that has gone silent", async () => {
    const { client, socket } = await joined();
    socket.onmessage?.({ data: JSON.stringify({ type: "keepalive" }) });
    vi.advanceTimersByTime(SILENCE_MS + SILENCE_MS / 6);
    expect(socket.closed).toBe(true);
    client.close();
  });

  it("leaves alone a hub that never sends keepalives", async () => {
    // An older hub: a quiet room must not look like a dead one.
    const { client, socket } = await joined();
    vi.advanceTimersByTime(SILENCE_MS * 3);
    expect(socket.closed).toBe(false);
    client.close();
  });
});
