import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import { mkdir, readFile, realpath } from "node:fs/promises";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { createServer } from "vite";
import { connect } from "./check-terminal-behavior.mjs";

async function until(predicate, message) {
  const deadline = Date.now() + 15000;
  while (!await predicate()) {
    assert(Date.now() < deadline, message);
    await delay(20);
  }
}

export async function checkRmuxIsolated({ workspace, baseUrl, project }) {
  const vite = await createServer({ root: project, logLevel: "error", server: { middlewareMode: true, watch: null } });
  let client;
  let hub;
  let receiver;
  let model;
  let id;
  try {
    const { TeraxGhostty } = await vite.ssrLoadModule("@terax/ghostty-core/adapted");
    const { AdaptedGhosttyTerminalModel } = await vite.ssrLoadModule("/src/modules/terminal/ghostty/AdaptedGhosttyTerminalModel.ts");
    const { PtyOutputHub } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputHub.ts");
    const { PtyOutputReceiver } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputReceiver.ts");
    const { tokenBytes } = await vite.ssrLoadModule("/src/modules/terminal/lib/ptyWire.ts");
    const bytes = await readFile(`${project}/packages/ghostty-core/adapted/ghostty-vt.wasm`);
    const core = await TeraxGhostty.loadBytes(Uint8Array.from(bytes).buffer);
    const session = `phantom-isolated-${randomUUID()}`;
    const openStream = async () => {
      client = await connect(baseUrl);
      hub = new PtyOutputHub({
        open: async () => tokenBytes(await client.invoke("pty_stream_open", {})),
        poll: (body, signal) => client.streamRequest("poll", body, signal),
        ack: (body) => client.streamRequest("ack", body),
      });
      await hub.ensureStream();
    };
    const receive = () => {
      model = new AdaptedGhosttyTerminalModel(core, { backend: "ghostty-webgl", cols: 80, rows: 24 });
      const decoder = new TextDecoder();
      const state = { tail: "", run: 0, longest: 0, total: 0, recentRun: 0, recentLongest: 0, query: false, code: undefined };
      let progress;
      receiver = new PtyOutputReceiver((chunk) => {
        model.write(chunk);
        for (const byte of chunk) {
          if (byte === 119) state.total++;
          state.run = byte === 119 ? state.run + 1 : 0;
          state.longest = Math.max(state.longest, state.run);
          state.recentRun = byte === 114 ? state.recentRun + 1 : 0;
          state.recentLongest = Math.max(state.recentLongest, state.recentRun);
        }
        const text = state.tail + decoder.decode(chunk, { stream: true });
        if (["\x1b[c", "\x1b[5n", "\x1b[6n"].some((query) => text.includes(query))) state.query = true;
        state.tail = text.slice(-2048);
      }, (parsed) => progress?.(parsed));
      progress = hub.register(id, receiver, (code) => { state.code = code; }).progress;
      receiver.start();
      return state;
    };
    await openStream();
    await assert.rejects(client.invoke("pty_open", { cols: 80, rows: 24, cwd: workspace, shell: "/bin/sh", session: "phantom-agent-reserved-check" }), /not a valid terminal session name/);
    id = await client.invoke("pty_open", { cols: 80, rows: 24, cwd: workspace, shell: "/bin/sh", blocks: false, session, presentation: "focused" });
    assert((await client.invoke("pty_sessions", {})).some((entry) => entry.name === session && entry.attached));
    const first = receive();
    const restoredCwd = join(await realpath(workspace), "reattached cwd");
    await mkdir(restoredCwd);
    const quotedCwd = `'${restoredCwd.replaceAll("'", "'\\''")}'`;
    const cwdSession = `${session}-cwd`;
    const cwdId = await client.invoke("pty_open", { cols: 80, rows: 24, cwd: workspace, shell: "/bin/sh", session: cwdSession });
    client.write(cwdId, `cd ${quotedCwd}; sleep 30\r`);
    await until(async () => await client.invoke("pty_get_cwd", { id: cwdId }) === restoredCwd, "rmux did not report the busy foreground cwd");
    await client.invoke("pty_close", { id: cwdId });
    const other = await connect(baseUrl);
    try {
      await assert.rejects(other.invoke("pty_get_cwd", { id: cwdId }), /no session/);
      const attachedId = await other.invoke("pty_attach", { name: cwdSession, cols: 80, rows: 24 });
      assert.equal(await other.invoke("pty_get_cwd", { id: attachedId }), restoredCwd);
      await other.invoke("pty_kill", { name: cwdSession });
      await other.invoke("pty_close", { id: attachedId });
    } finally {
      await other.close();
    }
    console.log("[rmux-isolated] PASS: busy foreground cwd survives reattach and remains connection-owned");
    client.write(id, "stty -echo; printf '\\033[c\\033[5n\\033[6n__RMUX_START__\\n'; head -c 262144 /dev/zero | tr '\\000' w; printf '\\n__RMUX_FLOOD_DONE__\\n'\r");
    await until(() => first.tail.includes("\r\n__RMUX_FLOOD_DONE__\r\n") && receiver.diagnostics().pendingBytes === 0, "rmux flood did not parse completely");
    assert.equal(first.longest, 262144, `received ${first.total} flood bytes in total`);
    assert.equal(first.query, false);
    client.write(id, "\x03\rhead -c 65536 /dev/zero | tr '\\000' r; printf '\\n__RMUX_RECENT_DONE__\\n'\r");
    await until(() => first.tail.includes("\r\n__RMUX_RECENT_DONE__\r\n") && receiver.diagnostics().pendingBytes === 0, "recent replay fixture did not finish");
    assert.equal(first.recentLongest, 65536);
    const initial = receiver.diagnostics();
    assert.equal(initial.receivedBytes, initial.processedBytes);
    receiver.dispose();
    receiver = null;
    hub.unregister(id);
    await client.invoke("pty_close", { id });
    id = undefined;
    assert((await client.invoke("pty_sessions", {})).some((entry) => entry.name === session && !entry.attached));
    hub.dispose();
    hub = null;
    model.dispose();
    model = null;
    await client.close();
    client = null;

    await openStream();
    assert((await client.invoke("pty_sessions", {})).some((entry) => entry.name === session && !entry.attached));
    id = await client.invoke("pty_attach", { name: session, cols: 80, rows: 24, presentation: "focused" });
    const replay = receive();
    await until(() => replay.tail.includes("\r\n__RMUX_RECENT_DONE__\r\n") && receiver.diagnostics().pendingBytes === 0, "rmux replay did not reach the previous tail");
    assert.equal(replay.recentLongest, 65536);
    assert.equal(replay.query, false);
    assert(model.readText(30).includes("__RMUX_RECENT_DONE__"));
    client.write(id, "printf '__RMUX_RECONNECTED__\\n'; exit 0\r");
    await until(() => replay.code !== undefined, "reattached rmux session did not exit");
    assert.equal(replay.code, 0);
    assert(replay.tail.includes("\r\n__RMUX_RECONNECTED__\r\n"));
    const done = receiver.diagnostics();
    assert.equal(done.pendingBytes, 0);
    assert.equal(done.receivedBytes, done.processedBytes);
    await client.invoke("pty_close", { id });
    id = undefined;
    console.log("[rmux-isolated] PASS: reserved-name rejection, unique stable session, lossless 256 KiB live flood, query filtering, detach survival, busy cwd recovery and connection ownership, fresh-WebSocket replay of the retained 64 KiB fixture and ordered exit (older output is bounded by daemon retention)");
  } finally {
    try {
      if (client && id !== undefined) await client.invoke("pty_close", { id }).catch(() => {});
      receiver?.dispose();
      hub?.dispose();
      model?.dispose();
      if (client) await client.close();
    } finally {
      await vite.close();
    }
  }
}
