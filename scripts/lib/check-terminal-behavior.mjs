import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";
import { createServer } from "vite";

async function until(predicate, message) {
  const deadline = Date.now() + 5000;
  while (!predicate()) {
    assert(Date.now() < deadline, message);
    await delay(20);
  }
}

export async function connect(baseUrl) {
  const ws = new WebSocket(`${baseUrl.replace(/^http/, "ws")}/ws`);
  ws.binaryType = "arraybuffer";
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("behavior WebSocket connect timed out")), 5000);
    ws.onopen = () => { clearTimeout(timeout); resolve(); };
    ws.onerror = () => { clearTimeout(timeout); reject(new Error("behavior WebSocket failed")); };
  }).catch((error) => { ws.close(); throw error; });
  let next = 1;
  const pending = new Map();
  const events = [];
  const complete = (id, value, error) => {
    const request = pending.get(id);
    if (!request) return;
    pending.delete(id);
    clearTimeout(request.timeout);
    request.cleanup();
    if (error) request.reject(error);
    else request.resolve(value);
  };
  ws.onmessage = ({ data }) => {
    if (data instanceof ArrayBuffer) {
      if (data.byteLength < 7) return;
      const view = new DataView(data);
      if (view.getUint8(0) !== 2) return;
      complete(view.getUint32(1, true), { status: view.getUint16(5, true), body: new Uint8Array(data, 7) });
      return;
    }
    let message;
    try { message = JSON.parse(data); } catch { return; }
    if (message.type === "result") complete(message.req, message.ok, message.err ? new Error(message.err) : null);
    else {
      if (events.length >= 256) events.shift();
      events.push(message);
    }
  };
  ws.onclose = () => {
    for (const id of pending.keys()) complete(id, null, new Error("behavior socket closed"));
  };
  const request = (send, signal) => new Promise((resolve, reject) => {
    if (signal?.aborted) { reject(new DOMException("Aborted", "AbortError")); return; }
    const id = next++;
    const abort = () => complete(id, null, new DOMException("Aborted", "AbortError"));
    const timeout = setTimeout(() => complete(id, null, new Error("behavior request timed out")), 5000);
    pending.set(id, { resolve, reject, timeout, cleanup: () => signal?.removeEventListener("abort", abort) });
    signal?.addEventListener("abort", abort, { once: true });
    try { send(id); } catch (error) { complete(id, null, error); }
  });
  return {
    events,
    invoke: (cmd, args) => request((req) => ws.send(JSON.stringify({ type: "invoke", req, cmd, args }))),
    streamRequest: (kind, body, signal) => request((req) => {
      const frame = new Uint8Array(5 + body.length);
      frame[0] = kind === "poll" ? 2 : 3;
      new DataView(frame.buffer).setUint32(1, req, true);
      frame.set(body, 5);
      ws.send(frame);
    }, signal),
    write: (id, text) => {
      const bytes = new TextEncoder().encode(text);
      const frame = new Uint8Array(5 + bytes.length);
      frame[0] = 1;
      new DataView(frame.buffer).setUint32(1, id, true);
      frame.set(bytes, 5);
      ws.send(frame);
    },
    close: async () => {
      for (const id of pending.keys()) complete(id, null, new Error("behavior client disposed"));
      ws.close();
      await until(() => ws.readyState === WebSocket.CLOSED, "behavior socket did not close");
    },
  };
}

export async function checkTerminalBehavior({ workspace, invoke, baseUrl, project }) {
  const vite = await createServer({ root: project, logLevel: "error", server: { middlewareMode: true, watch: null } });
  let client;
  let hub;
  let receiver;
  let model;
  let blocks;
  let id;
  try {
    client = await connect(baseUrl);
    const { TeraxGhostty } = await vite.ssrLoadModule("@terax/ghostty-core/adapted");
    const { AdaptedGhosttyTerminalModel } = await vite.ssrLoadModule("/src/modules/terminal/ghostty/AdaptedGhosttyTerminalModel.ts");
    const { GhosttySemanticEventRouter } = await vite.ssrLoadModule("/src/modules/terminal/ghostty/core/GhosttySemanticEventRouter.ts");
    const { GhosttyBlocks } = await vite.ssrLoadModule("/src/modules/terminal/ghostty/GhosttyBlocks.ts");
    const { PtyOutputHub } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputHub.ts");
    const { PtyOutputReceiver } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputReceiver.ts");
    const { tokenBytes } = await vite.ssrLoadModule("/src/modules/terminal/lib/ptyWire.ts");
    const bytes = await readFile(`${project}/packages/ghostty-core/adapted/ghostty-vt.wasm`);
    const core = await TeraxGhostty.loadBytes(Uint8Array.from(bytes).buffer);
    let cwd = "";
    const copied = [];
    const router = new GhosttySemanticEventRouter({ onCwd: (value) => { cwd = value; }, onClipboard: (text) => copied.push(text) });
    model = new AdaptedGhosttyTerminalModel(core, {
      backend: "ghostty-webgl", cols: 80, rows: 24,
      onEvent: (event) => { router.handle(event); blocks.handle(event, cwd); },
    });
    blocks = new GhosttyBlocks(model);
    hub = new PtyOutputHub({
      open: async () => tokenBytes(await client.invoke("pty_stream_open", {})),
      poll: (body, signal) => client.streamRequest("poll", body, signal),
      ack: (body) => client.streamRequest("ack", body),
    });
    await hub.ensureStream();
    id = await client.invoke("pty_open", { cols: 80, rows: 24, cwd: workspace, blocks: false, shell: "/bin/sh", presentation: "focused" });
    let progress;
    receiver = new PtyOutputReceiver((chunk) => {
      for (let i = 0; i < chunk.length; i += 7) model.write(chunk.subarray(i, i + 7));
    }, (parsed) => progress?.(parsed));
    let code;
    progress = hub.register(id, receiver, (status) => { code = status; }).progress;
    receiver.start();
    const osc = (value) => `\x1b]${value}\x07`;
    const payload = osc(`7;file://localhost${workspace}`) + osc("133;C;codex") +
      "semantic needle 中文\r\n" + osc("7;file://localhost/should-not-change") + osc("52;c;YmVoYXZpb3I=") +
      osc("777;notify;Terax;codex;attention") + osc("777;notify;Terax;codex;working") +
      osc("777;notify;Terax;codex;finished") + osc("133;D;0");
    const octal = [...Buffer.from(payload)].map((byte) => `\\${byte.toString(8).padStart(3, "0")}`).join("");
    client.write(id, `stty -echo; printf '${octal}'; exit 0\r`);
    await until(() => code !== undefined, "real PTY behavior check did not exit");
    assert.equal(code, 0);
    assert.equal(cwd, workspace);
    assert.deepEqual(copied, ["behavior"]);
    assert.deepEqual(blocks.readById("1"), { command: "codex", cwd: workspace, exitCode: 0, output: "semantic needle 中文" });
    assert(model.readText(300).includes("semantic needle 中文"));
    const diagnostics = receiver.diagnostics();
    assert.equal(diagnostics.pendingBytes, 0);
    assert.equal(diagnostics.receivedBytes, diagnostics.processedBytes);
    const signals = client.events.filter((event) => event.type === "agent_signal" && event.id === id);
    assert.deepEqual(signals.map((signal) => signal.kind), ["started", "attention", "working", "finished", "exited"]);
    assert.equal(signals[0].agent, "codex");
    await client.invoke("pty_close", { id });
    id = undefined;
    console.log("[terminal-behavior] PASS: real PTY, binary poll/ack, fragmented WASM parsing, OSC 7/52/133, command metadata and agent markers (no real coding-agent process)");

    const first = await invoke("room_post", { text: "integration behavior first", refs: [] });
    await until(() => client.events.some((event) => event.event === "room:event" && event.payload.message?.seq === first.seq), "Room broadcast did not arrive");
    const initial = await client.invoke("room_history", {});
    const broadcast = client.events.find((event) => event.event === "room:event" && event.payload.message?.seq === first.seq);
    assert.equal(broadcast.payload.epoch, initial.epoch);
    assert.equal(broadcast.payload.message.text, first.text);
    assert(initial.messages.some((message) => message.seq === first.seq && message.text === first.text));
    hub.dispose();
    hub = null;
    await client.close();
    client = null;
    const second = await invoke("room_post", { text: "integration behavior while disconnected", refs: [] });
    client = await connect(baseUrl);
    const replay = await client.invoke("room_history", { since: first.seq });
    assert.equal(replay.epoch, initial.epoch);
    assert.deepEqual(replay.messages.map((message) => message.seq), [second.seq]);
    assert.equal(replay.messages[0].text, second.text);
    console.log("[terminal-behavior] PASS: isolated Room post, live broadcast and since-sequence replay after a new WebSocket connection");

    for (const [name, body, type, inline] of [
      ["behavior.svg", '<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>', "image/svg+xml", true],
      ["behavior.html", "<script>alert(1)</script>", "application/octet-stream", false],
      ["behavior.png", "http-media-boundary-fixture", "image/png", true],
    ]) {
      const uploaded = await fetch(`${baseUrl}/files?${new URLSearchParams({ dir: workspace, name })}`, { method: "POST", body, signal: AbortSignal.timeout(5000) });
      assert.equal(uploaded.status, 200);
      const { path } = await uploaded.json();
      const media = await fetch(`${baseUrl}/files?${new URLSearchParams({ path, inline: "true" })}`, { signal: AbortSignal.timeout(5000) });
      assert.equal(media.status, 200);
      assert.equal(media.headers.get("content-type"), type);
      assert.equal(media.headers.get("x-content-type-options"), "nosniff");
      assert.equal(media.headers.get("content-disposition").startsWith("inline"), inline);
      if (inline) assert.equal(media.headers.get("content-security-policy"), "sandbox");
      assert.equal(await media.text(), body);
      const attachment = await fetch(`${baseUrl}/files?${new URLSearchParams({ path })}`, { signal: AbortSignal.timeout(5000) });
      assert.equal(attachment.status, 200);
      assert.equal(attachment.headers.get("content-type"), "application/octet-stream");
      assert(attachment.headers.get("content-disposition").startsWith("attachment;"));
      assert.equal(await attachment.text(), body);
    }
    console.log("[terminal-behavior] PASS: uploaded media bytes and inline HTTP allowlist/sandbox; HTML remains attachment (not a browser-rendering test)");
  } finally {
    if (client && id !== undefined) await client.invoke("pty_close", { id }).catch(() => {});
    receiver?.dispose();
    hub?.dispose();
    blocks?.dispose();
    model?.dispose();
    try {
      if (client) await client.close();
    } finally {
      await vite.close();
    }
  }
}
