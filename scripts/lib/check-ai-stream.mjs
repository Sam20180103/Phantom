import assert from "node:assert/strict";
import { createServer } from "node:http";
import { setTimeout as delay } from "node:timers/promises";

async function until(predicate, message) {
  const deadline = Date.now() + 5000;
  while (!predicate()) {
    assert(Date.now() < deadline, message);
    await delay(10);
  }
}

async function connect(baseUrl) {
  const ws = new WebSocket(baseUrl.replace(/^http/, "ws") + "/ws");
  ws.binaryType = "arraybuffer";
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  let next = 1;
  const pending = new Map();
  const packets = new Map();
  ws.onmessage = ({ data }) => {
    if (data instanceof ArrayBuffer) {
      assert(data.byteLength >= 10);
      const view = new DataView(data);
      assert.equal(view.getUint8(0), 4);
      packets.get(Number(view.getBigUint64(1, true)))?.push(new Uint8Array(data, 9));
      return;
    }
    const msg = JSON.parse(data);
    if (msg.type !== "result") return;
    const call = pending.get(msg.req);
    if (!call) return;
    pending.delete(msg.req);
    clearTimeout(call.timer);
    if (msg.err) call.reject(new Error(msg.err));
    else call.resolve(msg.ok);
  };
  ws.onclose = () => {
    for (const call of pending.values()) {
      clearTimeout(call.timer);
      call.reject(new Error("test socket closed"));
    }
    pending.clear();
  };
  const invoke = (cmd, args) => {
    const req = next++;
    const events = [];
    packets.set(req, events);
    const result = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        pending.delete(req);
        reject(new Error(cmd + " timed out"));
      }, 8000);
      pending.set(req, { resolve, reject, timer });
    });
    result.catch(() => {});
    ws.send(JSON.stringify({ type: "invoke", req, cmd, args }));
    return { result, events };
  };
  return { invoke, close: async () => {
    ws.close();
    await until(() => ws.readyState === WebSocket.CLOSED, "test socket did not close");
  } };
}

export async function checkAiStream({ baseUrl }) {
  const started = new Set();
  const closed = new Set();
  const total = 384 * 1024;
  const provider = createServer((req, res) => {
    started.add(req.url);
    res.on("close", () => closed.add(req.url));
    if (req.url === "/waiting") return;
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.flushHeaders();
    if (req.url === "/body") res.end(Buffer.alloc(total, 120));
  });
  await new Promise(resolve => provider.listen(0, "127.0.0.1", resolve));
  const url = "http://127.0.0.1:" + provider.address().port;
  const client = await connect(baseUrl);
  const other = await connect(baseUrl);
  const start = (path, requestId) => client.invoke("ai_http_stream", {
    requestId, url: url + path, method: "GET", allowPrivateNetwork: true,
  });
  const chunks = events => events.filter(packet => packet[0] === 1);
  const size = events => chunks(events).reduce((n, p) => n + p.length - 1, 0);
  try {
    const stream = start("/body", "body");
    await until(() => size(stream.events) > 64 * 1024, "stream did not fill its byte window");
    await delay(100);
    const stalled = size(stream.events);
    assert(stalled <= 128 * 1024, "stream exceeded its byte window");
    await delay(100);
    assert.equal(size(stream.events), stalled, "slow consumer must stop receiving");
    await assert.rejects(client.invoke("ai_http_ack", { requestId: "body", bytes: 1 }).result, /acknowledgment/);
    await other.invoke("ai_http_ack", { requestId: "body", bytes: 128 * 1024 }).result;
    await other.invoke("ai_http_cancel", { requestId: "body" }).result;
    await delay(100);
    assert.equal(size(stream.events), stalled, "another connection cannot control this stream");
    await client.invoke("ai_http_ack", { requestId: "body", bytes: size(stream.events) }).result;
    const deadline = Date.now() + 5000;
    while (!stream.events.some(p => p[0] === 2)) {
      assert(Date.now() < deadline, "acknowledged stream did not complete");
      await delay(10);
      await client.invoke("ai_http_ack", { requestId: "body", bytes: size(stream.events) }).result;
    }
    await stream.result;
    assert.equal(size(stream.events), total);
    assert(chunks(stream.events).every(p => p.length <= 65537 && p.subarray(1).every(b => b === 120)));

    const blocked = start("/cancel", "cancel");
    await until(() => blocked.events.some(p => p[0] === 0), "missing response headers");
    await client.invoke("ai_http_cancel", { requestId: "cancel" }).result;
    await blocked.result;
    await until(() => closed.has("/cancel"), "cancel did not close idle provider response");

    const waiting = start("/waiting", "waiting");
    await until(() => started.has("/waiting"), "provider did not receive request");
    await client.invoke("ai_http_cancel", { requestId: "waiting" }).result;
    await waiting.result;
    await until(() => closed.has("/waiting"), "cancel before headers did not close provider request");

    const disconnect = start("/disconnect", "disconnect");
    await until(() => disconnect.events.some(p => p[0] === 0), "missing disconnect-test headers");
    await client.close();
    await assert.rejects(disconnect.result, /socket closed/);
    await until(() => closed.has("/disconnect"), "disconnected client left provider request alive");
    console.log("[ai-stream-check] PASS: real WS binary payloads, byte-window stall/resume, ack validation, connection ownership, cancel before/after headers, disconnect cleanup");
  } finally {
    await Promise.all([client.close(), other.close()]);
    provider.closeAllConnections();
    await new Promise(resolve => provider.close(resolve));
  }
}
