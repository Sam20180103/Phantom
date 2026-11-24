import assert from "node:assert/strict";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { mkdtemp, mkdir, readFile, readdir, readlink, rm, symlink, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const project = fileURLToPath(new URL("../", import.meta.url));
const port = 19889;
async function requireFreePort() {
  const probe = createServer();
  await new Promise((resolve, reject) => {
    probe.once("error", reject);
    probe.listen(port, "127.0.0.1", resolve);
  });
  await new Promise((resolve, reject) => probe.close((error) => error ? reject(error) : resolve()));
}
await requireFreePort();

const build = spawn("cargo", ["build", "-p", "phantom-server", "--locked"], {
  cwd: project,
  env: { ...process.env, CARGO_TARGET_DIR: `${project}/target`, CARGO_PROFILE_DEV_DEBUG: "0", CARGO_INCREMENTAL: "0" },
  stdio: "inherit",
});
const [buildCode] = await once(build, "exit");
assert.equal(buildCode, 0, "isolated server build failed");
await requireFreePort();

const runtime = await mkdtemp(join(tmpdir(), "phantom-copy-check-"));
const rmuxMode = process.argv.includes("--rmux");
const rmuxSocket = join(runtime, `rmux-${process.getuid?.()}`, "default");
const rmuxProbe = `phantom-probe-${runtime.split("/").at(-1)}`;
let ownsRmux = false;
let child;
let exited;
let ws;
let logs = "";
const pending = new Map();
const events = [];
const workspace = `${runtime}/workspace`;
const dest = `${workspace}/dest`;
const source = `${runtime}/source`;

try {
  for (const dir of [dest, source, `${runtime}/outside`, `${runtime}/config`, `${runtime}/data`, `${runtime}/cache`, `${runtime}/web`]) {
    await mkdir(dir, { recursive: true });
  }
  await writeFile(`${source}/a.txt`, "new");
  await writeFile(`${dest}/a.txt`, "old");
  const fingerprint = `phantom-copy-check:${runtime}`;
  await writeFile(`${runtime}/web/index.html`, fingerprint);
  const backend = rmuxMode ? "rmux" : "local";
  await writeFile(`${runtime}/config.yaml`, `bind-addr: 127.0.0.1:${port}\nauth: none\npty-backend: ${backend}\nweb-root: ${runtime}/web\nroom:\n  auto-start: false\n  peek: false\n`);
  const env = {
    ...process.env,
    PHANTOM_CONFIG: `${runtime}/config.yaml`, PHANTOM_HOST: "127.0.0.1", PHANTOM_PORT: String(port),
    PHANTOM_PTY_BACKEND: backend, PHANTOM_WEB_ROOT: `${runtime}/web`,
    XDG_CONFIG_HOME: `${runtime}/config`, XDG_DATA_HOME: `${runtime}/data`, XDG_CACHE_HOME: `${runtime}/cache`,
  };
  delete env.PHANTOM_CERT;
  delete env.PHANTOM_CERT_KEY;
  if (rmuxMode) {
    env.RMUX_TMPDIR = runtime;
    env.RMUX_SDK_ENDPOINT = rmuxSocket;
    delete env.RMUX;
    delete env.TMUX;
    ownsRmux = true;
    const daemon = spawn("rmux", ["-S", rmuxSocket, "start-server"], { env, stdio: "inherit", timeout: 10000 });
    const [code] = await once(daemon, "exit");
    assert.equal(code, 0, "isolated rmux daemon did not start");
    const probe = spawn("rmux", ["-S", rmuxSocket, "new-session", "-d", "-s", rmuxProbe, "/bin/sh"], { env, stdio: "inherit", timeout: 10000 });
    const [probeCode] = await once(probe, "exit");
    assert.equal(probeCode, 0, "isolated rmux probe session did not start");
  }
  child = spawn(`${project}/target/debug/phantom-server`, [], { cwd: project, env, stdio: ["ignore", "pipe", "pipe"] });
  exited = new Promise((resolve) => {
    child.once("exit", resolve);
    child.once("error", resolve);
  });
  for (const stream of [child.stdout, child.stderr]) stream.on("data", (chunk) => { logs = (logs + chunk).slice(-65536); });

  let ready = false;
  for (let n = 0; n < 50; n++) {
    assert.equal(child.exitCode, null, `isolated server exited: ${logs}`);
    try {
      const response = await fetch(`http://127.0.0.1:${port}/`, { signal: AbortSignal.timeout(1000) });
      if (response.status === 200 && await response.text() === fingerprint) { ready = true; break; }
    } catch {}
    await delay(100);
  }
  assert(ready, `isolated server did not become ready: ${logs}`);
  ws = new WebSocket(`ws://127.0.0.1:${port}/ws`);
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error("WebSocket connect timed out")), 5000);
    ws.onopen = () => { clearTimeout(timeout); resolve(); };
    ws.onerror = (event) => { clearTimeout(timeout); reject(event); };
  });
  let nextReq = 1;
  ws.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.type === "event") {
      if (events.length >= 256) events.shift();
      events.push(message);
      return;
    }
    if (message.type !== "result") return;
    const request = pending.get(message.req);
    if (!request) return;
    pending.delete(message.req);
    clearTimeout(request.timeout);
    if (message.err) request.reject(new Error(message.err));
    else request.resolve(message.ok);
  };
  ws.onclose = () => {
    for (const request of pending.values()) {
      clearTimeout(request.timeout);
      request.reject(new Error("isolated socket closed"));
    }
    pending.clear();
  };
  const invoke = (cmd, args) => new Promise((resolve, reject) => {
    assert.equal(child.exitCode, null, "isolated server is no longer running");
    const req = nextReq++;
    const timeout = setTimeout(() => { pending.delete(req); reject(new Error(`${cmd} timed out`)); }, 5000);
    pending.set(req, { resolve, reject, timeout });
    ws.send(JSON.stringify({ type: "invoke", req, cmd, args }));
  });
  const args = { sources: [`${source}/a.txt`], destDir: dest, root: workspace };

  if (rmuxMode) {
    const sessions = await invoke("pty_sessions", {});
    assert.deepEqual(sessions.map((session) => session.name), [rmuxProbe], "test backend must use only its private rmux daemon");
  }

  await assert.rejects(invoke("fs_clipboard_paths", {}));

  await assert.rejects(invoke("fs_copy", args), /authorized workspace/);
  await invoke("workspace_authorize", { path: workspace });
  await assert.rejects(invoke("fs_copy", { sources: args.sources, destDir: dest }), /root/);
  const conflict = await invoke("fs_copy", args);
  assert.deepEqual(conflict.created, []);
  assert.equal(conflict.failed.length, 1);
  assert.equal(await readFile(`${dest}/a.txt`, "utf8"), "old");

  const copies = await Promise.all(Array.from({ length: 4 }, () => invoke("fs_copy", { ...args, renameOnConflict: true })));
  const created = copies.flatMap((result) => { assert.deepEqual(result.failed, []); return result.created; });
  assert.equal(new Set(created).size, 4);
  for (const path of created) assert.equal(await readFile(path, "utf8"), "new");
  assert.equal(await readFile(`${dest}/a.txt`, "utf8"), "old");
  const partial = await invoke("fs_copy", { ...args, sources: [`${source}/missing`, `${source}/a.txt`], renameOnConflict: true });
  assert.equal(partial.created.length, 1);
  assert.equal(partial.failed.length, 1);
  assert.equal(partial.failed[0].source, `${source}/missing`);
  const legacy = await invoke("fs_copy", { sources: args.sources, to: dest, root: workspace, renameOnConflict: true });
  assert.equal(legacy.created.length, 1);

  await symlink(`${runtime}/outside`, `${workspace}/escape`);
  await assert.rejects(invoke("fs_copy", { ...args, destDir: `${workspace}/escape` }), /authorized workspace/);
  await assert.rejects(invoke("fs_copy", { ...args, destDir: `${runtime}/outside` }), /authorized workspace/);
  await assert.rejects(invoke("fs_copy", { ...args, sources: Array(1025).fill(`${source}/a.txt`) }), /at most/);
  await mkdir(`${source}/d`);
  await symlink(".", `${source}/d/cycle`);
  await symlink("missing", `${source}/d/dangling`);
  const links = await invoke("fs_copy", { ...args, sources: [`${source}/d`] });
  assert.deepEqual(links.failed, []);
  assert.equal(await readlink(`${dest}/d/cycle`), ".");
  assert.equal(await readlink(`${dest}/d/dangling`), "missing");
  assert.deepEqual(await readdir(`${runtime}/outside`), []);
  assert(!(await readdir(dest)).some((name) => name.startsWith(".phantom-copy-")));
  if (process.argv.includes("--index")) {
    const { checkFileIndex } = await import("./lib/check-file-index.mjs");
    await checkFileIndex({ workspace, source, invoke, events, baseUrl: `http://127.0.0.1:${port}` });
  }
  if (process.argv.includes("--terminal")) {
    const { checkTerminalBehavior } = await import("./lib/check-terminal-behavior.mjs");
    await checkTerminalBehavior({ workspace, invoke, baseUrl: `http://127.0.0.1:${port}`, project });
  }
  if (process.argv.includes("--ai-stream")) {
    const { checkAiStream } = await import("./lib/check-ai-stream.mjs");
    await checkAiStream({ baseUrl: `http://127.0.0.1:${port}` });
  }
  if (rmuxMode) {
    const { checkRmuxIsolated } = await import("./lib/check-rmux-isolated.mjs");
    await checkRmuxIsolated({ workspace, baseUrl: `http://127.0.0.1:${port}`, project });
  }
  console.log("[fs-copy-check] PASS: desktop-only clipboard boundary, authorization, camelCase/legacy arguments, concurrent no-clobber copies, partial success, limits and symlink safety");
} finally {
  ws?.close();
  if (child && child.exitCode === null && child.signalCode === null) {
    child.kill("SIGTERM");
    const force = setTimeout(() => child.kill("SIGKILL"), 3000);
    await exited;
    clearTimeout(force);
  }
  if (ownsRmux) {
    const stop = spawn("rmux", ["-S", rmuxSocket, "kill-server"], { stdio: "ignore", timeout: 3000 });
    await once(stop, "exit").catch(() => {});
  }
  await rm(runtime, { recursive: true, force: true });
}
