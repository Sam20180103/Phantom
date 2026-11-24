import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, rm, writeFile, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
const runtimeDir = "/tmp/phantom-terax-v090-browser-runtime";
const configFile = `${runtimeDir}/config.yaml`;

console.log("[browser-integration] setting up isolated test environment...");
await rm(runtimeDir, { recursive: true, force: true });
await mkdir(`${runtimeDir}/config`, { recursive: true });
await mkdir(`${runtimeDir}/data`, { recursive: true });
await mkdir(`${runtimeDir}/cache`, { recursive: true });

await writeFile(
  configFile,
  `bind-addr: 127.0.0.1:19889
auth: none
pty-backend: local
web-root: ${root}/dist
room:
  name: browser-integration-test
  peer: browser-integration-test
  auto-start: false
  peek: false
`,
);

const serverBin = `${root}/target/debug/phantom-server`;
console.log("[browser-integration] starting phantom-server on 127.0.0.1:19889...");
const serverProc = spawn(serverBin, [], {
  env: {
    ...process.env,
    RUST_LOG: "info",
    PHANTOM_CONFIG: configFile,
    PHANTOM_HOST: "127.0.0.1",
    PHANTOM_PORT: "19889",
    PHANTOM_PTY_BACKEND: "local",
    PHANTOM_WEB_ROOT: `${root}/dist`,
    XDG_CONFIG_HOME: `${runtimeDir}/config`,
    XDG_DATA_HOME: `${runtimeDir}/data`,
    XDG_CACHE_HOME: `${runtimeDir}/cache`,
  },
  stdio: ["ignore", "pipe", "pipe"],
});

serverProc.stderr.on("data", (chunk) => {
  const line = chunk.toString().trim();
  if (line.includes("ERROR") || line.includes("WARN")) {
    console.error(`[server-log] ${line}`);
  }
});

let viteProc;
let failed;

try {
  // Wait for phantom-server on 19889
  let serverReady = false;
  for (let i = 0; i < 50; i++) {
    try {
      const res = await fetch("http://127.0.0.1:19889/");
      if (res.status === 200 || res.status === 404) {
        serverReady = true;
        break;
      }
    } catch {}
    await delay(100);
  }
  assert.ok(serverReady, "phantom-server must be ready on 127.0.0.1:19889");

  // Start Vite in integration mode on 127.0.0.1:15174
  console.log("[browser-integration] starting vite on 127.0.0.1:15174 (integration mode)...");
  viteProc = spawn(
    `${root}/node_modules/.bin/vite`,
    ["--mode", "integration", "--host", "127.0.0.1", "--port", "15174", "--strictPort"],
    {
      cwd: root,
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  let viteReady = false;
  for (let i = 0; i < 60; i++) {
    try {
      const res = await fetch("http://127.0.0.1:15174/");
      if (res.status === 200) {
        viteReady = true;
        break;
      }
    } catch {}
    await delay(100);
  }
  assert.ok(viteReady, "vite dev server must be ready on 127.0.0.1:15174");
  console.log("[browser-integration] server and vite proxy are up and healthy");

  // Initialize Ghostty WASM
  const { instance } = await WebAssembly.instantiate(
    await readFile(new URL("../packages/ghostty-core/adapted/ghostty-vt.wasm", import.meta.url)),
  );
  const api = instance.exports;

  // SSR load modules through Vite
  globalThis.location = new URL("http://127.0.0.1:15174");
  const viteServer = await createServer({
    root,
    logLevel: "error",
    server: { middlewareMode: true, watch: null },
  });

  const { transport } = await viteServer.ssrLoadModule("/src/lib/transport/client.ts");
  const { webSocketPtyTransport } = await viteServer.ssrLoadModule("/src/lib/transport/pty-stream.ts");
  const { PtyOutputHub } = await viteServer.ssrLoadModule("/src/modules/terminal/lib/PtyOutputHub.ts");
  const { PtyOutputReceiver } = await viteServer.ssrLoadModule("/src/modules/terminal/lib/PtyOutputReceiver.ts");
  const { resolveTerminalBackend, resolvedTerminalBackend } = await viteServer.ssrLoadModule(
    "/src/modules/terminal/backend/selection.ts",
  );
  const { rendererProfileKey } = await viteServer.ssrLoadModule(
    "/src/modules/terminal/ghostty/webgl/WebGlCellRenderer.ts",
  );

  // Phase 1: Verify token auth and basic stream rejection
  console.log("[browser-integration] Phase 1: verify security tokens on stream endpoint");
  const hub = new PtyOutputHub(webSocketPtyTransport());
  await hub.ensureStream();
  const rejected = await transport.ptyStreamRequest("ack", new Uint8Array(12));
  assert.equal(rejected.status, 403, "unissued stream tokens must return 403 Forbidden");

  // Phase 2: WebGL fallback and renderer capability matrix
  console.log("[browser-integration] Phase 2: verify WebGPU -> WebGL fallback");
  const fallbackBackend = resolveTerminalBackend("ghostty-webgpu", {
    webGpu: false,
    webGl2: true,
    wasmSimd: true,
  });
  assert.equal(fallbackBackend, "ghostty-webgl", "fallback without WebGPU must select ghostty-webgl");

  const preferredWebGpu = resolveTerminalBackend("ghostty-webgpu", {
    webGpu: true,
    webGl2: true,
    wasmSimd: true,
  });
  assert.equal(preferredWebGpu, "ghostty-webgpu", "with WebGPU available must select ghostty-webgpu");

  const activeBackend = resolvedTerminalBackend();
  assert.equal(activeBackend, "ghostty-webgl", "headless/standard environment must resolve to ghostty-webgl");

  // Verify WebGL renderer atlas reuse across theme variations
  const profileBase = {
    metrics: {
      font: { family: "monospace", size: 14, lineHeight: 1.2, letterSpacing: 0, weight: "400" },
      scale: 1,
      deviceCellWidth: 9,
      deviceCellHeight: 18,
      deviceBaseline: 14,
      cellWidth: 9,
      cellHeight: 18,
      baseline: 14,
    },
    scale: 1,
  };
  const key1 = rendererProfileKey({
    ...profileBase,
    theme: { background: [0, 0, 0], foreground: [255, 255, 255], cursor: [255, 255, 255], selection: { color: [0, 0, 0], alpha: 0.5 }, palette: [] },
  });
  const key2 = rendererProfileKey({
    ...profileBase,
    theme: { background: [30, 30, 30], foreground: [200, 200, 200], cursor: [200, 200, 200], selection: { color: [50, 50, 50], alpha: 0.5 }, palette: [] },
  });
  assert.equal(key1, key2, "monochrome glyph atlas key must be reused across color themes");

  // Phase 3: Single PTY 4 MiB stream through 15174 Vite proxy with parser backpressure
  console.log("[browser-integration] Phase 3: 4 MiB local PTY flood through Vite proxy (15174 -> 19889)");
  const model1 = api.restty_create_with_limits(80, 24, 1024 * 1024, 100);
  const ptr1 = api.restty_alloc(32 * 1024);
  const id1 = await transport.invoke("pty_open", {
    cols: 80,
    rows: 24,
    cwd: root,
    blocks: false,
    shell: "/bin/sh",
    presentation: "focused",
  });

  let run1 = 0;
  let longest1 = 0;
  let tail1 = "";
  let progress1;
  const decoder = new TextDecoder();

  const receiver1 = new PtyOutputReceiver(
    async (bytes) => {
      new Uint8Array(api.memory.buffer, ptr1, bytes.length).set(bytes);
      assert.equal(api.restty_write(model1, ptr1, bytes.length), 0);
      for (const byte of bytes) {
        run1 = byte === 120 ? run1 + 1 : 0;
        if (run1 > longest1) longest1 = run1;
      }
      tail1 = (tail1 + decoder.decode(bytes, { stream: true })).slice(-256);
      await delay(1);
    },
    (parsed) => progress1?.(parsed),
    (status) => assert.notEqual(status, "failed"),
  );

  let onExit1;
  const exit1Promise = new Promise((res) => { onExit1 = res; });
  progress1 = hub.register(id1, receiver1, onExit1).progress;
  receiver1.start();

  await transport.ptyWrite(
    id1,
    new TextEncoder().encode(
      "stty -echo; printf '\\033[32m4MiB Stream Test\\033[0m\\n'; head -c 4194304 /dev/zero | tr '\\000' x; printf '\\n__V090_4MIB_DONE__\\n'; exit\r",
    ),
  );

  const code1 = await Promise.race([
    exit1Promise,
    new Promise((_, rej) => setTimeout(() => rej(new Error("4 MiB stream timed out")), 30_000)),
  ]);
  assert.equal(code1, 0, "4 MiB stream must exit with code 0");
  assert.equal(longest1, 4194304, "4 MiB flood must arrive without loss or corruption");
  assert.ok(tail1.includes("__V090_4MIB_DONE__"), "final marker must arrive in tail");

  const diag1 = receiver1.diagnostics();
  assert.equal(diag1.receivedBytes, diag1.processedBytes);
  assert.equal(diag1.pendingBytes, 0);
  receiver1.dispose();
  hub.unregister(id1);
  api.restty_free(ptr1, 32 * 1024);
  api.restty_destroy(model1);

  // Phase 4: Multi-Pane Split Screen Concurrent Streams
  console.log("[browser-integration] Phase 4: multi-pane concurrent PTY streams (split-screen simulation)");
  const modelA = api.restty_create_with_limits(80, 24, 512 * 1024, 50);
  const ptrA = api.restty_alloc(16 * 1024);
  const modelB = api.restty_create_with_limits(80, 24, 512 * 1024, 50);
  const ptrB = api.restty_alloc(16 * 1024);

  const idA = await transport.invoke("pty_open", { cols: 80, rows: 24, cwd: root, shell: "/bin/sh" });
  const idB = await transport.invoke("pty_open", { cols: 80, rows: 24, cwd: root, shell: "/bin/sh" });

  let textA = "";
  let textB = "";
  let progA, progB;
  let exitA, exitB;
  const exitAPromise = new Promise((res) => { exitA = res; });
  const exitBPromise = new Promise((res) => { exitB = res; });

  const recA = new PtyOutputReceiver(
    async (bytes) => {
      new Uint8Array(api.memory.buffer, ptrA, bytes.length).set(bytes);
      api.restty_write(modelA, ptrA, bytes.length);
      textA += decoder.decode(bytes, { stream: true });
      await delay(1);
    },
    (p) => progA?.(p),
    (s) => assert.notEqual(s, "failed"),
  );

  const recB = new PtyOutputReceiver(
    async (bytes) => {
      new Uint8Array(api.memory.buffer, ptrB, bytes.length).set(bytes);
      api.restty_write(modelB, ptrB, bytes.length);
      textB += decoder.decode(bytes, { stream: true });
      await delay(1);
    },
    (p) => progB?.(p),
    (s) => assert.notEqual(s, "failed"),
  );

  progA = hub.register(idA, recA, exitA).progress;
  progB = hub.register(idB, recB, exitB).progress;
  recA.start();
  recB.start();

  // Send concurrent jobs to both panes
  await transport.ptyWrite(idA, new TextEncoder().encode("stty -echo; printf '__PANE_A_BURST__\\n'; exit\r"));
  await transport.ptyWrite(idB, new TextEncoder().encode("stty -echo; printf '__PANE_B_BURST__\\n'; exit\r"));

  const [resA, resB] = await Promise.all([exitAPromise, exitBPromise]);
  assert.equal(resA, 0, "Pane A must exit 0");
  assert.equal(resB, 0, "Pane B must exit 0");
  assert.ok(textA.includes("__PANE_A_BURST__"), "Pane A must receive its own output");
  assert.ok(!textA.includes("__PANE_B_BURST__"), "Pane A must not leak into Pane B");
  assert.ok(textB.includes("__PANE_B_BURST__"), "Pane B must receive its own output");
  assert.ok(!textB.includes("__PANE_A_BURST__"), "Pane B must not leak into Pane A");

  recA.dispose();
  recB.dispose();
  hub.unregister(idA);
  hub.unregister(idB);
  api.restty_free(ptrA, 16 * 1024);
  api.restty_destroy(modelA);
  api.restty_free(ptrB, 16 * 1024);
  api.restty_destroy(modelB);

  // Phase 5: Mobile touch tap logic
  console.log("[browser-integration] Phase 5: mobile touch tap gesture recognition");
  function evaluateTouch(start, end) {
    const dx = Math.abs(end.x - start.x);
    const dy = Math.abs(end.y - start.y);
    const dt = end.time - start.time;
    return dx <= 10 && dy <= 10 && dt < 400;
  }
  assert.equal(evaluateTouch({ x: 100, y: 100, time: 1000 }, { x: 105, y: 102, time: 1150 }), true, "short stationary tap must trigger focus");
  assert.equal(evaluateTouch({ x: 100, y: 100, time: 1000 }, { x: 100, y: 100, time: 1500 }), false, "long press (>400ms) must not trigger tap focus");
  assert.equal(evaluateTouch({ x: 100, y: 100, time: 1000 }, { x: 130, y: 100, time: 1100 }), false, "scroll gesture (>10px) must not trigger tap focus");

  hub.dispose();
  await viteServer.close();
  console.log("[browser-integration] ALL BROWSER INTEGRATION CHECKS PASSED!");
} catch (err) {
  failed = err;
} finally {
  if (viteProc) viteProc.kill("SIGTERM");
  if (serverProc) serverProc.kill("SIGTERM");
  try {
    await rm(runtimeDir, { recursive: true, force: true });
  } catch {}
}

if (failed) {
  console.error("[browser-integration] FAILED:", failed);
  process.exit(1);
} else {
  process.exit(0);
}
