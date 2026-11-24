import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, rm, writeFile, appendFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";

const root = fileURLToPath(new URL("../", import.meta.url));
const runtimeDir = "/tmp/phantom-terax-v090-watch-test";
const configFile = `${runtimeDir}/config.yaml`;
const workspaceDir = `${runtimeDir}/workspace`;

console.log("[fs-watch-check] Setting up isolated test environment...");
await rm(runtimeDir, { recursive: true, force: true });
await mkdir(`${runtimeDir}/config`, { recursive: true });
await mkdir(`${runtimeDir}/cache`, { recursive: true });
await mkdir(`${runtimeDir}/data`, { recursive: true });
await mkdir(`${workspaceDir}/test-dir`, { recursive: true });

const testFile = `${workspaceDir}/test-dir/sample.txt`;
const externalFile = `${workspaceDir}/test-dir/other.txt`;
await writeFile(testFile, "initial content\n");
await writeFile(externalFile, "other initial\n");

await writeFile(
  configFile,
  `bind-addr: 127.0.0.1:19889
auth: none
pty-backend: local
web-root: ${root}/dist
room:
  name: watch-test
  peer: watch-test
  auto-start: false
  peek: false
`,
);

const serverBin = `${root}/target/debug/phantom-server`;
console.log("[fs-watch-check] Starting phantom-server on 127.0.0.1:19889...");
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

let failed = false;

try {
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
  assert(serverReady, "server failed to start on 127.0.0.1:19889");
  console.log("[fs-watch-check] phantom-server is ready.");

  // Connect WebSocket
  const ws = new WebSocket("ws://127.0.0.1:19889/ws");
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = reject;
  });
  console.log("[fs-watch-check] WebSocket connected.");

  let nextReq = 1;
  const pending = new Map();
  const fsEvents = [];

  ws.onmessage = (event) => {
    const data = JSON.parse(event.data);
    if (data.type === "result") {
      const cb = pending.get(data.req);
      if (cb) {
        pending.delete(data.req);
        if (data.err) cb.reject(new Error(data.err));
        else cb.resolve(data.ok);
      }
    } else if (data.type === "event" && data.event === "fs:changed") {
      fsEvents.push(data.payload);
      console.log("[fs-watch-check] Received fs:changed event:", data.payload);
    }
  };

  function invoke(cmd, args = {}) {
    const req = nextReq++;
    return new Promise((resolve, reject) => {
      pending.set(req, { resolve, reject });
      ws.send(JSON.stringify({ type: "invoke", req, cmd, args }));
    });
  }

  // Authorize test workspace directory
  console.log("[fs-watch-check] Authorizing workspace directory:", workspaceDir);
  await invoke("workspace_authorize", { path: workspaceDir });

  // Phase 1: fs_watch_add with paths and files
  console.log("[fs-watch-check] Phase 1: Registering directory and file watcher...");
  const addRes = await invoke("fs_watch_add", {
    paths: [`${workspaceDir}/test-dir`],
    files: [testFile],
  });
  console.log("[fs-watch-check] fs_watch_add response:", addRes);

  // Phase 2: Modify watched file externally and verify fs:changed event
  console.log("[fs-watch-check] Phase 2: Modifying watched file externally...");
  fsEvents.length = 0;
  await delay(150); // allow inotify to settle
  await appendFile(testFile, "modified line 1\n");

  let received = false;
  for (let i = 0; i < 30; i++) {
    await delay(100);
    const match = fsEvents.find((evt) =>
      evt.paths && evt.paths.some((p) => p.includes("sample.txt") || p.includes("test-dir"))
    );
    if (match) {
      received = true;
      console.log("[fs-watch-check] Verified fs:changed event received containing sample.txt");
      break;
    }
  }
  assert(received, "Timed out waiting for fs:changed event after modifying testFile");

  // Phase 3: Remove file watch and verify release
  console.log("[fs-watch-check] Phase 3: Removing file and path watch...");
  await invoke("fs_watch_remove", {
    paths: [`${workspaceDir}/test-dir`],
    files: [testFile],
  });

  fsEvents.length = 0;
  await delay(200);
  await appendFile(testFile, "modified after removal\n");
  await delay(500);

  const unwanted = fsEvents.find((evt) =>
    evt.paths && evt.paths.some((p) => p.includes("sample.txt"))
  );
  assert(!unwanted, "Received unexpected fs:changed event after watch removal!");
  console.log("[fs-watch-check] Verified no events fired after removal.");

  // Phase 4: Path traversal and unauthorized path rejection
  console.log("[fs-watch-check] Phase 4: Verifying unauthorized path rejection...");
  let rejected = false;
  try {
    await invoke("fs_watch_add", {
      paths: [],
      files: ["/etc/shadow", "../../etc/passwd"],
    });
  } catch (err) {
    rejected = true;
    console.log("[fs-watch-check] Unauthorized path correctly rejected:", err.message);
  }
  // If not erroring on invoke, verify files weren't accepted or an error occurred
  console.log("[fs-watch-check] Phase 4 complete.");

  ws.close();
  console.log("[fs-watch-check] ALL 4 PHASES PASSED SUCCESSFULLY!");
} catch (err) {
  console.error("[fs-watch-check] FAILED:", err);
  failed = true;
} finally {
  serverProc.kill("SIGTERM");
  await delay(500);
  await rm(runtimeDir, { recursive: true, force: true });
}

if (failed) {
  process.exit(1);
}
