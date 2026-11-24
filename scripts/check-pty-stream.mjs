import assert from "node:assert/strict";
import { lstat, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
assert.ok((await lstat(new URL("../.git", import.meta.url))).isFile(), "Run from the integration worktree");
globalThis.location = new URL("http://127.0.0.1:15174");
const vite = await createServer({ root, logLevel: "error", server: { middlewareMode: true, watch: null } });
const { instance } = await WebAssembly.instantiate(await readFile(new URL("../packages/ghostty-core/adapted/ghostty-vt.wasm", import.meta.url)));
const api = instance.exports;
const model = api.restty_create_with_limits(80, 24, 1024 * 1024, 100);
assert.ok(model);
const pointer = api.restty_alloc(32 * 1024);
assert.ok(pointer);
let hub;
let receiver;
let transport;
let id;
let timeout;
let failed;
try {
  ({ transport } = await vite.ssrLoadModule("/src/lib/transport/client.ts"));
  const { webSocketPtyTransport } = await vite.ssrLoadModule("/src/lib/transport/pty-stream.ts");
  const { PtyOutputHub } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputHub.ts");
  const { PtyOutputReceiver } = await vite.ssrLoadModule("/src/modules/terminal/lib/PtyOutputReceiver.ts");
  hub = new PtyOutputHub(webSocketPtyTransport());
  await hub.ensureStream();
  const rejected = await transport.ptyStreamRequest("ack", new Uint8Array(12));
  assert.equal(rejected.status, 403, "unissued stream tokens must be rejected");
  id = await transport.invoke("pty_open", { cols: 80, rows: 24, cwd: root, blocks: false, shell: "/bin/sh", presentation: "focused" });
  let run = 0;
  let longest = 0;
  let tail = "";
  let progress;
  const decoder = new TextDecoder();
  receiver = new PtyOutputReceiver(async (bytes) => {
    new Uint8Array(api.memory.buffer, pointer, bytes.length).set(bytes);
    assert.equal(api.restty_write(model, pointer, bytes.length), 0);
    for (const byte of bytes) { run = byte === 120 ? run + 1 : 0; longest = Math.max(longest, run); }
    tail = (tail + decoder.decode(bytes, { stream: true })).slice(-256);
    await delay(1);
  }, (parsed) => progress?.(parsed), (status) => { assert.notEqual(status, "failed"); });
  const exit = new Promise((resolve) => {
    progress = hub.register(id, receiver, resolve).progress;
  });
  receiver.start();
  await transport.ptyWrite(id, new TextEncoder().encode("stty -echo; printf '\\033[31m流控\\033[0m\\n'; head -c 4194304 /dev/zero | tr '\\000' x; printf '\\n__V090_DONE__\\n'; exit\r"));
  const code = await Promise.race([exit, new Promise((_, reject) => {
    timeout = setTimeout(() => reject(new Error("PTY smoke test timed out")), 30_000);
  })]);
  assert.equal(code, 0);
  assert.equal(longest, 4194304, "flood must arrive without lost or duplicated bytes");
  assert.ok(tail.includes("__V090_DONE__"), "exit must follow the final output");
  const diagnostics = receiver.diagnostics();
  assert.equal(diagnostics.receivedBytes, diagnostics.processedBytes);
  assert.equal(diagnostics.pendingBytes, 0);
  console.log("4 MiB PTY flood, Ghostty parsing, parser acknowledgements and ordered exit passed");
} catch (error) {
  failed = error;
} finally {
  clearTimeout(timeout);
  if (id !== undefined) await transport.invoke("pty_close", { id }).catch(() => {});
  receiver?.dispose();
  hub?.dispose();
  api.restty_free(pointer, 32 * 1024);
  api.restty_destroy(model);
  await vite.close();
}
if (failed) console.error(failed);
process.exit(failed ? 1 : 0);
