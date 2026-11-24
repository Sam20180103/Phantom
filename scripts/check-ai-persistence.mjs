import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, readFile, rm, stat, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import { createServer } from "vite";

const root = fileURLToPath(new URL("../", import.meta.url));
const runtimeDir = "/tmp/phantom-terax-v090-ai-test";
const configFile = `${runtimeDir}/config.yaml`;
const dataDir = `${runtimeDir}/data`;

console.log("[ai-check] setting up isolated test environment...");
await rm(runtimeDir, { recursive: true, force: true });
await mkdir(`${runtimeDir}/config`, { recursive: true });
await mkdir(dataDir, { recursive: true });
await mkdir(`${runtimeDir}/cache`, { recursive: true });

await writeFile(
  configFile,
  `bind-addr: 127.0.0.1:19889
auth: none
pty-backend: local
web-root: ${root}/dist
room:
  name: ai-test
  peer: ai-test
  auto-start: false
  peek: false
`,
);

const serverBin = `${root}/target/debug/phantom-server`;
console.log("[ai-check] starting phantom-server on 127.0.0.1:19889...");
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
    XDG_DATA_HOME: dataDir,
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

let failed;

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
  assert.ok(serverReady, "phantom-server must be ready on 127.0.0.1:19889");

  globalThis.location = new URL("http://127.0.0.1:19889");
  const vite = await createServer({
    root,
    logLevel: "error",
    server: { middlewareMode: true, watch: null },
  });

  const { transport } = await vite.ssrLoadModule("/src/lib/transport/client.ts");

  console.log("[ai-check] Phase 1: save valid conversation and verify atomic disk file");
  const sessionId = "session_test_abc-123";
  const messagesV1 = [
    { id: "m1", role: "user", content: "hello ai persistence test" },
    { id: "m2", role: "assistant", content: "response 1" },
  ];
  await transport.invoke("ai_session_save", {
    id: sessionId,
    content: JSON.stringify(messagesV1),
    migrate: false,
    durable: true,
  });

  const expectedFile = `${dataDir}/phantom/conversations/session-${sessionId}.json`;
  const fileStat1 = await stat(expectedFile);
  assert.ok(fileStat1.isFile(), "session file must exist on disk");
  const onDiskContent1 = await readFile(expectedFile, "utf-8");
  assert.deepEqual(JSON.parse(onDiskContent1), messagesV1, "disk content must match saved JSON");

  console.log("[ai-check] Phase 2: load saved session and test missing session");
  const loadedV1 = await transport.invoke("ai_session_load", { id: sessionId });
  assert.ok(typeof loadedV1 === "string", "load must return string");
  assert.deepEqual(JSON.parse(loadedV1), messagesV1, "loaded content must match saved version");

  const missingResult = await transport.invoke("ai_session_load", { id: "non-existent-session-id" });
  assert.equal(missingResult, null, "loading non-existent session must return null");

  console.log("[ai-check] Phase 3: update session and verify .bak rotation");
  const messagesV2 = [
    ...messagesV1,
    { id: "m3", role: "user", content: "second turn" },
  ];
  await transport.invoke("ai_session_save", {
    id: sessionId,
    content: JSON.stringify(messagesV2),
    migrate: false,
    durable: true,
  });

  const onDiskContent2 = await readFile(expectedFile, "utf-8");
  assert.deepEqual(JSON.parse(onDiskContent2), messagesV2, "disk content must reflect updated version");

  const expectedBakFile = `${dataDir}/phantom/conversations/session-${sessionId}.json.bak`;
  const bakStat = await stat(expectedBakFile);
  assert.ok(bakStat.isFile(), "backup file (.bak) must exist after update");
  const onDiskBak = await readFile(expectedBakFile, "utf-8");
  assert.deepEqual(JSON.parse(onDiskBak), messagesV1, "backup content must contain previous version");

  console.log("[ai-check] Phase 4: security validation and path traversal rejection");
  // Non-array JSON
  await assert.rejects(
    async () => {
      await transport.invoke("ai_session_save", {
        id: "bad-content-1",
        content: JSON.stringify({ notAnArray: true }),
        migrate: false,
        durable: false,
      });
    },
    /array/,
    "non-array messages must be rejected",
  );

  // Invalid JSON
  await assert.rejects(
    async () => {
      await transport.invoke("ai_session_save", {
        id: "bad-content-2",
        content: "[invalid-json]",
        migrate: false,
        durable: false,
      });
    },
    /(array|JSON)/,
    "invalid JSON must be rejected",
  );

  // Path traversal attempts
  const maliciousIds = [
    "../../etc/passwd",
    "../session-escape",
    "sub/dir/id",
    "test\\backslash",
    "null\0byte",
    "",
    "a".repeat(129),
  ];

  for (const badId of maliciousIds) {
    await assert.rejects(
      async () => {
        await transport.invoke("ai_session_save", {
          id: badId,
          content: "[]",
          migrate: false,
          durable: false,
        });
      },
      /invalid conversation id/,
      `path traversal id "${badId}" must be rejected by save`,
    );

    await assert.rejects(
      async () => {
        await transport.invoke("ai_session_load", { id: badId });
      },
      /invalid conversation id/,
      `path traversal id "${badId}" must be rejected by load`,
    );

    await assert.rejects(
      async () => {
        await transport.invoke("ai_session_delete", { id: badId });
      },
      /invalid conversation id/,
      `path traversal id "${badId}" must be rejected by delete`,
    );
  }

  console.log("[ai-check] Phase 5: delete session and verify disk cleanup");
  await transport.invoke("ai_session_delete", { id: sessionId });

  const fileExistsAfterDelete = await stat(expectedFile).then(() => true).catch(() => false);
  assert.equal(fileExistsAfterDelete, false, "main session file must be deleted");

  const bakExistsAfterDelete = await stat(expectedBakFile).then(() => true).catch(() => false);
  assert.equal(bakExistsAfterDelete, false, "backup session file must be deleted");

  const reloadAfterDelete = await transport.invoke("ai_session_load", { id: sessionId });
  assert.equal(reloadAfterDelete, null, "loading deleted session must return null");

  await vite.close();
  console.log("[ai-check] ALL AI SESSION PERSISTENCE TESTS PASSED!");
} catch (err) {
  failed = err;
} finally {
  serverProc.kill("SIGTERM");
  try {
    await rm(runtimeDir, { recursive: true, force: true });
  } catch {}
}

if (failed) {
  console.error("[ai-check] FAILED:", failed);
  process.exit(1);
} else {
  process.exit(0);
}
