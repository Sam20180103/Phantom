import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdir, readFile, symlink, writeFile } from "node:fs/promises";
import { setTimeout as delay } from "node:timers/promises";
import { promisify } from "node:util";

export async function checkFileIndex({ workspace, source, invoke, events, baseUrl }) {
  const root = `${workspace}/index-case`;
  await mkdir(`${root}/src/deep`, { recursive: true });
  await mkdir(`${root}/node_modules/pkg`, { recursive: true });
  await promisify(execFile)("git", ["init", "--quiet", root]);
  await writeFile(`${root}/alpha.txt`, "alpha");
  await writeFile(`${root}/src/needle.rs`, "needle");
  await writeFile(`${root}/.hidden.txt`, "hidden");
  await writeFile(`${root}/node_modules/pkg/needle.rs`, "pruned");
  const search = (query) => invoke("fs_search", { root, query });
  const list = (extra = {}) => invoke("fs_list_files", { root, ...extra });

  assert.equal((await search("alpha")).hits.length, 1);
  await writeFile(`${root}/late-alpha.txt`, "late");
  assert.equal((await search("alpha")).hits.length, 1, "the warm query must reuse its cached walk");
  await invoke("fs_create_file", { path: `${root}/trigger.txt` });
  assert.equal((await search("alpha")).hits.length, 2, "a mutation must invalidate an unwatched root");
  const initial = await list();
  assert(initial.files.includes("src/needle.rs"));
  assert(!initial.files.some((path) => path.includes("node_modules") || path === ".hidden.txt"));
  assert.equal((await list({ limit: initial.files.length })).truncated, false);
  assert.equal((await list({ limit: initial.files.length - 1 })).truncated, true);
  assert(!(await list({ maxDepth: 1 })).files.includes("src/needle.rs"));
  assert((await list({ showHidden: true })).files.includes(".hidden.txt"));

  await invoke("fs_rename", { from: `${root}/alpha.txt`, to: `${root}/renamed.txt` });
  assert.equal((await search("renamed")).hits[0].path, `${root}/renamed.txt`);
  await invoke("fs_write_file", { path: `${root}/from-write.txt`, content: "written" });
  assert((await list()).files.includes("from-write.txt"));
  await invoke("fs_copy", { sources: [`${source}/a.txt`], destDir: root, root: workspace });
  assert((await list()).files.includes("a.txt"));
  await invoke("fs_delete", { path: `${root}/a.txt` });
  assert(!(await list()).files.includes("a.txt"));
  await invoke("fs_create_dir", { path: `${root}/new/deep` });
  assert((await search("new/deep")).hits.some((hit) => hit.rel === "new/deep" && hit.is_dir));

  await invoke("fs_write_file", { path: `${root}/.gitignore`, content: "renamed.txt\n" });
  assert(!(await list()).files.includes("renamed.txt"));
  await invoke("fs_write_file", { path: `${root}/.gitignore`, content: "" });
  assert((await list()).files.includes("renamed.txt"));
  await mkdir(`${root}/.git/info`, { recursive: true });
  await invoke("fs_write_file", { path: `${root}/.git/info/exclude`, content: "trigger.txt\n" });
  assert(!(await list()).files.includes("trigger.txt"));
  const nestedList = () => invoke("fs_list_files", { root: `${root}/src` });
  assert((await nestedList()).files.includes("needle.rs"));
  await invoke("fs_write_file", { path: `${root}/.gitignore`, content: "src/needle.rs\n" });
  assert(!(await nestedList()).files.includes("needle.rs"), "ancestor ignore edits must invalidate nested roots");
  await invoke("fs_write_file", { path: `${root}/.gitignore`, content: "" });
  assert((await nestedList()).files.includes("needle.rs"));
  const queries = await Promise.all(Array.from({ length: 4 }, () => search("needle")));
  for (const result of queries) assert.deepEqual(result.hits.map((hit) => hit.rel), ["src/needle.rs"]);

  const upload = async (name, body) => {
    const response = await fetch(`${baseUrl}/files?${new URLSearchParams({ dir: root, name })}`, { method: "POST", body });
    const text = await response.text();
    assert.equal(response.status, 200, text);
    return JSON.parse(text);
  };
  await writeFile(`${root}/shared.txt`, "original");
  await invoke("fs_create_file", { path: `${root}/refresh-trigger` });
  assert.equal((await search("shared")).hits.length, 1);
  const uploads = await Promise.all(Array.from({ length: 4 }, (_, n) => upload("shared.txt", `upload-${n}`)));
  assert.equal(new Set(uploads.map((item) => item.path)).size, 4);
  for (const [n, item] of uploads.entries()) assert.equal(await readFile(item.path, "utf8"), `upload-${n}`);
  assert.equal(await readFile(`${root}/shared.txt`, "utf8"), "original");
  assert.equal((await search("shared")).hits.length, 5, "HTTP writes must invalidate the shared index");
  await writeFile(`${source}/upload-victim`, "keep");
  await symlink(`${source}/upload-victim`, `${root}/link.txt`);
  const linkUpload = await upload("link.txt", "new");
  assert.equal(linkUpload.name, "link-1.txt");
  assert.equal(await readFile(`${source}/upload-victim`, "utf8"), "keep");
  const download = await fetch(`${baseUrl}/files?${new URLSearchParams({ path: linkUpload.path })}`);
  assert.equal(await download.text(), "new");
  assert.equal(download.headers.get("x-content-type-options"), "nosniff");
  assert(download.headers.get("content-disposition").startsWith("attachment;"));
  const rejected = await fetch(`${baseUrl}/files?${new URLSearchParams({ dir: root, name: "../escape" })}`, { method: "POST", body: "no" });
  assert.equal(rejected.status, 400);

  assert.equal((await search("before-watch")).hits.length, 0);
  await writeFile(`${root}/before-watch.txt`, "external");
  assert.equal((await search("before-watch")).hits.length, 0);
  await invoke("fs_watch_add", { paths: [root] });
  assert.equal((await search("before-watch")).hits.length, 1, "rearming must discard an unwatched snapshot");
  assert.equal((await search("watched-added")).hits.length, 0);
  events.length = 0;
  await writeFile(`${root}/watched-added.txt`, "external");
  const deadline = Date.now() + 4000;
  while (!events.some((event) => event.event === "fs:changed" && event.payload.paths.includes(root))) {
    assert(Date.now() < deadline, "directory change event timed out");
    await delay(30);
  }
  assert.equal((await search("watched-added")).hits.length, 1, "the event must invalidate before it is delivered");
  await invoke("fs_watch_remove", { paths: [root] });
  console.log("[fs-index-check] PASS: cached queries, depth/visibility/limits, all mutation paths, ignore rules, HTTP no-clobber upload/download and watcher invalidation");
}
