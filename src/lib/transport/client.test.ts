import { afterEach, beforeEach, expect, it, vi } from "vitest";

class Socket {
  static all: Socket[] = [];
  sent: unknown[] = [];
  binaryType = "";
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  constructor() {
    Socket.all.push(this);
  }
  send(data: unknown) {
    this.sent.push(data);
  }
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  Socket.all = [];
  vi.stubGlobal("WebSocket", Socket);
  vi.stubGlobal("location", { protocol: "http:", host: "127.0.0.1:19889" });
});

afterEach(() => {
  vi.clearAllTimers();
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it("correlates binary stream replies without changing the payload", async () => {
  const { transport } = await import("@/lib/transport/client");
  const body = Uint8Array.of(0, 255, 128, 27);
  const pending = transport.ptyStreamRequest("poll", body);
  const socket = Socket.all[0];
  socket.onopen?.();
  await Promise.resolve();
  const request = new DataView(socket.sent[0] as ArrayBuffer);
  expect(request.getUint8(0)).toBe(2);
  expect(new Uint8Array(request.buffer, 5)).toEqual(body);
  const reply = new Uint8Array(7 + body.length);
  reply[0] = 2;
  const header = new DataView(reply.buffer);
  header.setUint32(1, request.getUint32(1, true), true);
  header.setUint16(5, 200, true);
  reply.set(body, 7);
  socket.onmessage?.({ data: reply.buffer });
  await expect(pending).resolves.toMatchObject({ status: 200, body });
});

it("aborts while connecting without sending the abandoned poll", async () => {
  const { transport } = await import("@/lib/transport/client");
  const controller = new AbortController();
  const pending = transport.ptyStreamRequest(
    "poll",
    new Uint8Array(12),
    controller.signal,
  );
  const rejected = expect(pending).rejects.toMatchObject({
    name: "AbortError",
  });
  controller.abort();
  await rejected;
  Socket.all[0].onopen?.();
  await Promise.resolve();
  expect(Socket.all[0].sent).toHaveLength(0);
});

it("rejects pending poll and acknowledgement when the socket closes", async () => {
  const { transport } = await import("@/lib/transport/client");
  const poll = transport.ptyStreamRequest("poll", new Uint8Array(12));
  const ack = transport.ptyStreamRequest("ack", new Uint8Array(12));
  const rejected = Promise.all([
    expect(poll).rejects.toThrow("phantom: disconnected"),
    expect(ack).rejects.toThrow("phantom: disconnected"),
  ]);
  Socket.all[0].onclose?.();
  await rejected;
});

it("routes binary AI packets by their 64-bit request id and cleans up on disconnect", async () => {
  const { transport, Channel } = await import("@/lib/transport/client");
  const channel = new Channel<ArrayBuffer>();
  channel.onmessage = vi.fn();
  const pending = transport.invoke("ai_http_stream", { onEvent: channel });
  const rejected = expect(pending).rejects.toThrow("phantom: disconnected");
  const socket = Socket.all[0];
  socket.onopen?.();
  await Promise.resolve();
  const { req } = JSON.parse(socket.sent[0] as string);
  const frame = new Uint8Array(13);
  frame[0] = 4;
  new DataView(frame.buffer).setBigUint64(1, BigInt(req), true);
  frame.set([1, 0, 128, 255], 9);
  socket.onmessage?.({ data: frame.buffer });
  expect(channel.onmessage).toHaveBeenCalledExactlyOnceWith(Uint8Array.of(1, 0, 128, 255).buffer);
  socket.onclose?.();
  await rejected;
  socket.onmessage?.({ data: frame.buffer });
  expect(channel.onmessage).toHaveBeenCalledTimes(1);
});
