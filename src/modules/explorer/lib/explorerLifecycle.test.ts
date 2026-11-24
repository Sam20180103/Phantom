import type { EffectCallback } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useFileTree, type DirEntry } from "@/modules/explorer/lib/useFileTree";
import { useExplorerFileDrop } from "@/modules/explorer/lib/useExplorerFileDrop";
import { useExplorerUpload } from "@/modules/explorer/lib/useExplorerUpload";
import { useExplorerClipboard } from "@/modules/explorer/lib/useExplorerClipboard";
import { copiedSelection, useCopiedSelection } from "@/modules/explorer/lib/useCopiedSelection";
import { useSidebarPanel } from "@/modules/sidebar/useSidebarPanel";
import type { WorkspaceEnv } from "@/modules/workspace";

type HookSlot = {
  value?: unknown;
  deps?: readonly unknown[];
  cleanup?: () => void;
};
type HookEffect = { slot: HookSlot; callback: EffectCallback };

const h = vi.hoisted(() => {
  let cursor = 0;
  const slots: HookSlot[] = [];
  const layouts: HookEffect[] = [];
  const effects: HookEffect[] = [];
  const takeSlot = () => {
    const index = cursor++;
    slots[index] ??= {};
    return slots[index];
  };
  const changed = (slot: HookSlot, deps?: readonly unknown[]) =>
    !deps || !slot.deps || deps.length !== slot.deps.length || deps.some((dep, i) => !Object.is(dep, slot.deps?.[i]));
  const memo = (factory: () => unknown, deps?: readonly unknown[]) => {
    const slot = takeSlot();
    if (changed(slot, deps)) {
      slot.value = factory();
      slot.deps = deps;
    }
    return slot.value;
  };
  const effect = (queue: HookEffect[], callback: EffectCallback, deps?: readonly unknown[]) => {
    const slot = takeSlot();
    if (!changed(slot, deps)) return;
    slot.deps = deps;
    queue.push({ slot, callback });
  };
  return {
    invoke: vi.fn(), add: vi.fn(), remove: vi.fn(), listen: vi.fn(),
    dragListen: vi.fn(), filesFrom: vi.fn(), uploadFiles: vi.fn(), pickFiles: vi.fn(), copyError: vi.fn(),
    workspace: { kind: "local" } as WorkspaceEnv,
    native: false,
    prefs: { showHidden: false, explorerGitDecorations: false },
    hooks: {
      useState(initial: unknown) {
        const slot = takeSlot();
        if (!("value" in slot)) slot.value = typeof initial === "function" ? initial() : initial;
        return [slot.value, (value: unknown) => {
          slot.value = typeof value === "function" ? value(slot.value) : value;
        }];
      },
      useRef: (current: unknown) => memo(() => ({ current }), []),
      useMemo: memo,
      useCallback: (callback: unknown, deps?: readonly unknown[]) => memo(() => callback, deps),
      useEffect: (callback: EffectCallback, deps?: readonly unknown[]) => effect(effects, callback, deps),
      useLayoutEffect: (callback: EffectCallback, deps?: readonly unknown[]) => effect(layouts, callback, deps),
    },
    render<T>(hook: () => T): T {
      cursor = 0;
      const value = hook();
      for (const queue of [layouts, effects]) {
        const pending = queue.splice(0);
        for (const { slot } of pending) slot.cleanup?.();
        for (const { slot, callback } of pending) slot.cleanup = callback() || undefined;
      }
      return value;
    },
    unmount() {
      for (const slot of slots) slot.cleanup?.();
      slots.length = 0;
      layouts.length = 0;
      effects.length = 0;
    },
  };
});

vi.mock("react", () => h.hooks);
vi.mock("@tauri-apps/api/core", () => ({ invoke: h.invoke, isTauri: () => h.native }));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({ onDragDropEvent: h.dragListen }),
}));
vi.mock("sonner", () => ({ toast: {
  loading: vi.fn(), success: vi.fn(), error: h.copyError, warning: vi.fn(), message: vi.fn(),
} }));
vi.mock("@/modules/workspace", () => ({
  currentWorkspaceEnv: () => h.workspace,
  useWorkspaceEnvStore: (selector: (state: { env: WorkspaceEnv }) => unknown) => selector({ env: h.workspace }),
  workspaceScopeKey: (env: WorkspaceEnv) => env.kind === "local" ? "local" : `wsl:${env.distro}`,
}));
vi.mock("@/modules/settings/preferences", () => ({
  usePreferencesStore: (selector: (state: typeof h.prefs) => unknown) => selector(h.prefs),
}));
vi.mock("@/modules/explorer/lib/watch", () => ({
  watchAdd: h.add, watchRemove: h.remove, listenFsChanged: h.listen,
}));
vi.mock("@/modules/explorer/lib/fileTransfer", () => ({
  filesFrom: h.filesFrom, uploadFiles: h.uploadFiles, pickFiles: h.pickFiles,
}));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<T>((ok, fail) => { resolve = ok; reject = fail; });
  return { promise, resolve, reject };
}

function entry(name: string, kind: "file" | "dir" = "file"): DirEntry {
  return { name, kind, size: 0, mtime: 0, gitignored: false };
}

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

beforeEach(() => {
  vi.clearAllMocks();
  h.workspace = { kind: "local" };
  h.native = false;
  h.prefs = { showHidden: false, explorerGitDecorations: false };
  h.invoke.mockImplementation(async () => []);
  h.listen.mockImplementation(async () => vi.fn());
  h.dragListen.mockImplementation(async () => vi.fn());
});
afterEach(() => {
  h.unmount();
  vi.unstubAllGlobals();
});

describe("file tree visibility lifecycle", () => {
  it("reacts to sidebar collapse edges without duplicate persistence or directory reads", async () => {
    const storage = { getItem: vi.fn(() => null), setItem: vi.fn() };
    vi.stubGlobal("window", { localStorage: storage });
    const explorerRef = { current: null };
    const render = () => h.render(() => {
      const sidebar = useSidebarPanel(explorerRef);
      useFileTree("/sidebar-edge", { active: !sidebar.sidebarCollapsed });
      return sidebar;
    });
    let sidebar = render();
    await settle();
    sidebar = render();
    h.invoke.mockClear();
    sidebar.persistSidebarCollapsed(true);
    sidebar = render();
    expect(sidebar.sidebarCollapsed).toBe(true);
    expect(h.remove).toHaveBeenCalledWith(["/sidebar-edge"], [], h.workspace);
    sidebar.persistSidebarCollapsed(true);
    render();
    expect(storage.setItem).toHaveBeenCalledExactlyOnceWith("terax.sidebar.collapsed", "1");
    expect(h.invoke).not.toHaveBeenCalled();
    sidebar.persistSidebarCollapsed(false);
    render();
    expect(h.invoke).toHaveBeenCalledOnce();
  });

  it("does not resurrect a deleted subtree when its old listing arrives late", async () => {
    h.invoke.mockImplementation(async () => [entry("src", "dir")]);
    h.render(() => useFileTree("/deleted"));
    await settle();
    const tree = h.render(() => useFileTree("/deleted"));
    const child = deferred<DirEntry[]>();
    h.invoke.mockImplementationOnce(() => child.promise);
    tree.expand("/deleted/src");
    h.render(() => useFileTree("/deleted"));
    h.invoke.mockResolvedValueOnce([]);
    tree.refresh("/deleted");
    await settle();
    child.resolve([entry("obsolete")]);
    await settle();
    const fresh = h.render(() => useFileTree("/deleted"));
    expect(fresh.nodes["/deleted/src"]).toBeUndefined();
    expect(fresh.expanded.has("/deleted/src")).toBe(false);
    expect(h.remove).toHaveBeenCalledWith(["/deleted/src"], [], h.workspace);
  });

  it("does not re-acquire the previous root when it changes while hidden", async () => {
    h.render(() => useFileTree("/hidden-old", { active: false }));
    h.render(() => useFileTree("/hidden-new", { active: false }));
    expect(h.invoke).not.toHaveBeenCalled();
    h.render(() => useFileTree("/hidden-new"));
    await settle();
    expect(h.add).toHaveBeenCalledExactlyOnceWith(["/hidden-new"], [], h.workspace);
    expect(h.invoke.mock.calls.map(([, args]) => args.path)).toEqual(["/hidden-new"]);
  });

  it("keeps the newest directory response when requests complete out of order", async () => {
    const old = deferred<DirEntry[]>();
    h.invoke.mockImplementationOnce(() => old.promise);
    const tree = h.render(() => useFileTree("/ordered"));
    h.invoke.mockResolvedValueOnce([entry("fresh")]);
    tree.refresh("/ordered");
    await settle();
    old.resolve([entry("obsolete")]);
    await settle();
    const fresh = h.render(() => useFileTree("/ordered"));
    expect(fresh.nodes["/ordered"]).toEqual({ status: "loaded", entries: [entry("fresh")] });
  });

  it("does no native work while initially hidden and resyncs once on show", async () => {
    h.render(() => useFileTree("/initial", { active: false }));
    expect(h.invoke).not.toHaveBeenCalled();
    expect(h.add).not.toHaveBeenCalled();
    expect(h.listen).not.toHaveBeenCalled();
    h.render(() => useFileTree("/initial", { active: true }));
    await settle();
    h.render(() => useFileTree("/initial", { active: true }));
    expect(h.add).toHaveBeenCalledExactlyOnceWith(["/initial"], [], h.workspace);
    expect(h.invoke).toHaveBeenCalledOnce();
    expect(h.listen).toHaveBeenCalledOnce();
  });

  it("releases ownership on hide and reloads the retained tree with current preferences", async () => {
    h.invoke.mockImplementation(async (_command, args) => args.path === "/visible" ? [entry("src", "dir")] : []);
    h.render(() => useFileTree("/visible"));
    await settle();
    let tree = h.render(() => useFileTree("/visible"));
    tree.expand("/visible/src");
    await settle();
    h.render(() => useFileTree("/visible"));
    const staleHandler = h.listen.mock.calls[0][0];
    h.invoke.mockClear();
    tree = h.render(() => useFileTree("/visible", { active: false }));
    expect(h.remove).toHaveBeenCalledExactlyOnceWith(["/visible", "/visible/src"], [], h.workspace);
    expect(tree.expanded.has("/visible/src")).toBe(true);
    staleHandler(["/visible/src/file.ts"]);
    tree.refresh("/visible");
    h.prefs.showHidden = true;
    h.render(() => useFileTree("/visible", { active: false }));
    expect(h.invoke).not.toHaveBeenCalled();
    h.render(() => useFileTree("/visible"));
    await settle();
    h.render(() => useFileTree("/visible"));
    expect(h.invoke.mock.calls.map(([, args]) => args.path)).toEqual(["/visible", "/visible/src"]);
    expect(h.invoke.mock.calls.every(([, args]) => args.showHidden)).toBe(true);
  });

  it.each(["resolve", "reject"] as const)("ignores a late %s from an old root", async (outcome) => {
    const pending = deferred<DirEntry[]>();
    h.invoke.mockImplementationOnce(() => pending.promise);
    h.render(() => useFileTree(`/old-${outcome}`));
    h.render(() => useFileTree(`/new-${outcome}`));
    await settle();
    if (outcome === "resolve") pending.resolve([entry("obsolete")]);
    else pending.reject(new Error("old failure"));
    await settle();
    const tree = h.render(() => useFileTree(`/new-${outcome}`));
    expect(Object.keys(tree.nodes)).toEqual([`/new-${outcome}`]);
    expect(tree.nodes[`/new-${outcome}`].status).toBe("loaded");
  });

  it("does not accept a pre-hide response after showing again", async () => {
    const pending = deferred<DirEntry[]>();
    h.invoke.mockImplementationOnce(() => pending.promise);
    h.render(() => useFileTree("/reopen"));
    h.render(() => useFileTree("/reopen", { active: false }));
    h.render(() => useFileTree("/reopen"));
    await settle();
    pending.resolve([entry("obsolete")]);
    await settle();
    const tree = h.render(() => useFileTree("/reopen"));
    expect(tree.nodes["/reopen"]).toEqual({ status: "loaded", entries: [] });
  });

  it("releases the original workspace and does not reuse its expanded paths in WSL", async () => {
    const local = h.workspace;
    h.invoke.mockImplementation(async () => [entry("src", "dir")]);
    h.render(() => useFileTree("/scope"));
    await settle();
    const tree = h.render(() => useFileTree("/scope"));
    tree.expand("/scope/src");
    await settle();
    h.render(() => useFileTree("/scope"));
    h.workspace = { kind: "wsl", distro: "test" };
    h.invoke.mockClear();
    h.render(() => useFileTree("/scope"));
    expect(h.remove).toHaveBeenCalledWith(["/scope", "/scope/src"], [], local);
    expect(h.add).toHaveBeenLastCalledWith(["/scope"], [], h.workspace);
    expect(h.invoke.mock.calls.map(([, args]) => [args.path, args.workspace])).toEqual([["/scope", h.workspace]]);
  });

  it("releases an event subscription that resolves after unmount", async () => {
    const pending = deferred<() => void>();
    const stop = vi.fn();
    h.listen.mockImplementationOnce(() => pending.promise);
    h.render(() => useFileTree("/unmount"));
    const staleHandler = h.listen.mock.calls[0][0];
    await settle();
    h.unmount();
    h.invoke.mockClear();
    pending.resolve(stop);
    await settle();
    staleHandler(["/unmount/file.ts"]);
    expect(stop).toHaveBeenCalledOnce();
    expect(h.invoke).not.toHaveBeenCalled();
  });

  it("restores only the matching workspace expansion after rapid hidden switches", async () => {
    const local = h.workspace;
    h.invoke.mockImplementation(async () => [entry("src", "dir")]);
    h.render(() => useFileTree("/cached-scope"));
    await settle();
    const tree = h.render(() => useFileTree("/cached-scope"));
    tree.expand("/cached-scope/src");
    await settle();
    h.render(() => useFileTree("/cached-scope"));
    h.render(() => useFileTree("/cached-scope", { active: false }));
    h.workspace = { kind: "wsl", distro: "test" };
    h.render(() => useFileTree("/cached-scope", { active: false }));
    h.workspace = local;
    h.render(() => useFileTree("/cached-scope", { active: false }));
    h.invoke.mockClear();
    h.render(() => useFileTree("/cached-scope"));
    expect(h.invoke.mock.calls.map(([, args]) => args.path)).toEqual(["/cached-scope", "/cached-scope/src"]);
  });
});

describe("clipboard lifecycle", () => {
  it("selects only the created paths after a successful refresh", async () => {
    h.native = true;
    h.invoke.mockResolvedValueOnce(["/source/a", "/source/b"]).mockResolvedValueOnce({
      created: ["/repo/a copy", "/repo/b"], failed: [],
    });
    const refresh = deferred<boolean>();
    const onCreated = vi.fn();
    const paste = h.render(() => useExplorerClipboard({
      rootPath: "/repo", enabled: true, refresh: () => refresh.promise, onCreated,
    }));
    const result = paste("/repo");
    await settle();
    expect(onCreated).not.toHaveBeenCalled();
    refresh.resolve(true);
    await result;
    expect(onCreated).toHaveBeenCalledExactlyOnceWith(["/repo/a copy", "/repo/b"]);
  });

  it.each(["false", "root", "workspace", "hide", "unmount"])("does not select after clipboard refresh ends with %s", async (change) => {
    h.native = true;
    h.invoke.mockResolvedValueOnce(["/source/a"]).mockResolvedValueOnce({ created: ["/repo/a"], failed: [] });
    const refresh = deferred<boolean>();
    const options = { rootPath: "/repo", enabled: true, refresh: () => refresh.promise, onCreated: vi.fn() };
    const paste = h.render(() => useExplorerClipboard(options));
    const result = paste("/repo");
    await settle();
    if (change === "unmount") h.unmount();
    else if (change !== "false") {
      if (change === "workspace") h.workspace = { kind: "wsl", distro: "other" };
      h.render(() => useExplorerClipboard({ ...options, rootPath: change === "root" ? "/other" : "/repo", enabled: change !== "hide" }));
    }
    refresh.resolve(change !== "false");
    expect(await result).toEqual([]);
    expect(options.onCreated).not.toHaveBeenCalled();
  });

  it("coalesces repeated paste requests while a native read is pending", async () => {
    h.native = true;
    const pending = deferred<string[]>();
    h.invoke.mockImplementationOnce(() => pending.promise).mockResolvedValueOnce({ created: ["/repo/a"], failed: [] });
    const refresh = vi.fn(async () => true);
    const paste = h.render(() => useExplorerClipboard({ rootPath: "/repo", enabled: true, refresh }));
    const first = paste("/repo");
    expect(await paste("/repo")).toEqual([]);
    expect(h.invoke).toHaveBeenCalledOnce();
    pending.resolve(["/source/a"]);
    expect(await first).toEqual(["/repo/a"]);
    expect(refresh).toHaveBeenCalledOnce();
  });

  it.each(["root", "workspace", "hide", "unmount"])("invalidates a pending clipboard read on %s", async (change) => {
    h.native = true;
    const pending = deferred<string[]>();
    h.invoke.mockImplementationOnce(() => pending.promise);
    const options = { rootPath: "/repo", enabled: true, refresh: vi.fn(async () => true) };
    const paste = h.render(() => useExplorerClipboard(options));
    const result = paste("/repo");
    if (change === "unmount") h.unmount();
    else {
      if (change === "workspace") h.workspace = { kind: "wsl", distro: "other" };
      h.render(() => useExplorerClipboard({ ...options, rootPath: change === "root" ? "/other" : "/repo", enabled: change !== "hide" }));
    }
    pending.resolve(["/source/a"]);
    expect(await result).toEqual([]);
    expect(h.invoke).toHaveBeenCalledOnce();
    expect(options.refresh).not.toHaveBeenCalled();
  });
});

describe("copied selection", () => {
  it("combines native paste, destination expansion and committed tree rows before selecting", async () => {
    h.native = true;
    let copied = false;
    h.invoke.mockImplementation(async (command, args) => {
      if (command === "fs_clipboard_paths") return ["/source/a", "/source/b"];
      if (command === "fs_copy") {
        copied = true;
        return { created: ["/paste-combined/dest/b", "/paste-combined/dest/a"], failed: [{ source: "/missing", message: "missing" }] };
      }
      if (args.path === "/paste-combined") return [entry("dest", "dir")];
      return copied ? [entry("a"), entry("b")] : [];
    });
    const onSelect = vi.fn();
    const render = () => h.render(() => {
      const tree = useFileTree("/paste-combined");
      const dest = tree.nodes["/paste-combined/dest"];
      const paths = tree.expanded.has("/paste-combined/dest") && dest?.status === "loaded"
        ? dest.entries.map((item) => `/paste-combined/dest/${item.name}`)
        : [];
      const onCreated = useCopiedSelection({ rootPath: "/paste-combined", enabled: true, entryPaths: paths, onSelect });
      const paste = useExplorerClipboard({ rootPath: "/paste-combined", enabled: true, refresh: (path) => tree.refresh(path, true), onCreated });
      return { tree, paste };
    });
    render();
    await settle();
    const { paste } = render();
    await paste("/paste-combined/dest");
    expect(onSelect).not.toHaveBeenCalled();
    const { tree } = render();
    expect(tree.expanded.has("/paste-combined/dest")).toBe(true);
    expect(onSelect).toHaveBeenCalledExactlyOnceWith(["/paste-combined/dest/a", "/paste-combined/dest/b"]);
  });

  it("keeps visible results in tree order, normalizes separators and ignores failed or hidden paths", () => {
    expect(copiedSelection(["C:\\repo\\b", "C:/repo/a", "C:/repo/a", "C:/repo/.hidden"], ["C:/repo/a", "C:/repo/b", "C:/repo/not-created"]))
      .toEqual(["C:/repo/a", "C:/repo/b"]);
  });

  it("waits for the refreshed rows to commit before applying one selection", () => {
    const onSelect = vi.fn();
    const options = { rootPath: "/repo", enabled: true, entryPaths: ["/repo/old"], onSelect };
    const select = h.render(() => useCopiedSelection(options));
    select(["/repo/b", "/repo/a"]);
    expect(onSelect).not.toHaveBeenCalled();
    h.render(() => useCopiedSelection({ ...options, entryPaths: ["/repo/a", "/repo/b"] }));
    expect(onSelect).toHaveBeenCalledExactlyOnceWith(["/repo/a", "/repo/b"]);
    h.render(() => useCopiedSelection(options));
    expect(onSelect).toHaveBeenCalledOnce();
  });

  it("does not clear an existing selection when no copied result is visible", () => {
    const options = { rootPath: "/repo", enabled: true, entryPaths: ["/repo/old"], onSelect: vi.fn() };
    const select = h.render(() => useCopiedSelection(options));
    select(["/repo/.hidden"]);
    h.render(() => useCopiedSelection(options));
    expect(options.onSelect).not.toHaveBeenCalled();
  });

  it.each(["root", "workspace", "hide", "unmount"])("discards queued selection on %s", (change) => {
    const options = { rootPath: "/repo", enabled: true, entryPaths: ["/repo/a"], onSelect: vi.fn() };
    const select = h.render(() => useCopiedSelection(options));
    select(["/repo/a"]);
    if (change === "unmount") h.unmount();
    else {
      if (change === "workspace") h.workspace = { kind: "wsl", distro: "other" };
      h.render(() => useCopiedSelection({ ...options, rootPath: change === "root" ? "/other" : "/repo", enabled: change !== "hide" }));
    }
    expect(options.onSelect).not.toHaveBeenCalled();
  });
});

describe("hidden explorer drop listeners", () => {
  it("cancels a browser file picker when the workspace changes before selection", async () => {
    const pending = deferred<File[]>();
    h.pickFiles.mockImplementationOnce(() => pending.promise);
    const options = { containerRef: { current: null }, rootPath: "/repo", selectedPath: null, isDir: () => false, onUploaded: vi.fn() };
    const upload = h.render(() => useExplorerUpload(options));
    const result = upload.pickAndUpload("/repo");
    h.workspace = { kind: "wsl", distro: "other" };
    pending.resolve([{ name: "a.txt" } as File]);
    await result;
    expect(h.uploadFiles).not.toHaveBeenCalled();
  });
  it("does not upload clipboard files pasted into rename or search inputs", () => {
    const handlers = new Map<string, (event: unknown) => void>();
    const element = { addEventListener: (name: string, handler: (event: unknown) => void) => handlers.set(name, handler), removeEventListener: vi.fn() };
    h.render(() => useExplorerUpload({ containerRef: { current: element as unknown as HTMLElement }, rootPath: "/repo", selectedPath: null, isDir: () => false, onUploaded: vi.fn() }));
    handlers.get("paste")?.({ target: { tagName: "INPUT" } });
    expect(h.filesFrom).not.toHaveBeenCalled();
    expect(h.uploadFiles).not.toHaveBeenCalled();
  });
  function nativeDrop() {
    vi.stubGlobal("window", { innerWidth: 100, innerHeight: 100 });
    vi.stubGlobal("document", { elementFromPoint: () => ({
      closest: () => ({ getAttribute: () => "/copy-root/dest" }),
    }) });
    return { payload: { type: "drop", paths: ["/source/a"], position: { x: 10, y: 20 } } };
  }

  it("passes the captured root and refreshes partial native-copy success", async () => {
    const event = nativeDrop();
    const onCopied = vi.fn();
    h.invoke.mockResolvedValueOnce({ created: ["/copy-root/dest/a"], failed: [{ source: "/source/missing", message: "not found" }] });
    h.render(() => useExplorerFileDrop({ rootPath: "/copy-root", isDir: () => true, onCopied }));
    h.dragListen.mock.calls[0][0](event);
    await settle();
    expect(h.invoke).toHaveBeenCalledExactlyOnceWith("fs_copy", {
      sources: ["/source/a"], destDir: "/copy-root/dest", root: "/copy-root", workspace: h.workspace,
    });
    expect(onCopied).toHaveBeenCalledExactlyOnceWith("/copy-root/dest");
    expect(h.copyError).toHaveBeenCalledWith("Some items could not be copied", { description: "/source/missing: not found" });
  });

  it("does not refresh when every native copy failed", async () => {
    const event = nativeDrop();
    const onCopied = vi.fn();
    h.invoke.mockResolvedValueOnce({ created: [], failed: [{ source: "/source/a", message: "occupied" }] });
    h.render(() => useExplorerFileDrop({ rootPath: "/copy-root", isDir: () => true, onCopied }));
    h.dragListen.mock.calls[0][0](event);
    await settle();
    expect(onCopied).not.toHaveBeenCalled();
    expect(h.copyError).toHaveBeenCalledWith("Copy failed", { description: "/source/a: occupied" });
  });

  it("selects partial native-copy success only after refreshing the destination", async () => {
    const event = nativeDrop();
    const refreshed = deferred<boolean>();
    const onCreated = vi.fn();
    h.invoke.mockResolvedValueOnce({ created: ["/copy-root/dest/a"], failed: [{ source: "/missing", message: "missing" }] });
    h.render(() => useExplorerFileDrop({ rootPath: "/copy-root", isDir: () => true, onCopied: () => refreshed.promise, onCreated }));
    h.dragListen.mock.calls[0][0](event);
    await settle();
    expect(onCreated).not.toHaveBeenCalled();
    refreshed.resolve(true);
    await settle();
    expect(onCreated).toHaveBeenCalledExactlyOnceWith(["/copy-root/dest/a"]);
  });

  it.each(["false", "reject", "root", "workspace", "hide", "unmount"])("does not select native-copy results after refresh ends with %s", async (change) => {
    const event = nativeDrop();
    const refreshed = deferred<boolean>();
    const options = { rootPath: "/copy-root", isDir: () => true, onCopied: vi.fn(() => refreshed.promise), onCreated: vi.fn() };
    h.invoke.mockResolvedValueOnce({ created: ["/copy-root/dest/a"], failed: [] });
    h.render(() => useExplorerFileDrop(options));
    h.dragListen.mock.calls[0][0](event);
    await settle();
    if (change === "unmount") h.unmount();
    else if (change === "root" || change === "workspace" || change === "hide") {
      if (change === "workspace") h.workspace = { kind: "wsl", distro: "other" };
      h.render(() => useExplorerFileDrop({ ...options, rootPath: change === "root" ? "/other" : "/copy-root", enabled: change !== "hide" }));
    }
    if (change === "reject") refreshed.reject(new Error("refresh failed"));
    else refreshed.resolve(change !== "false");
    await settle();
    expect(options.onCreated).not.toHaveBeenCalled();
    if (change === "reject") expect(h.copyError).toHaveBeenCalledWith("Files copied, but directory refresh failed: Error: refresh failed");
  });

  it("does not revive a native copy when switching away and back to the same root", async () => {
    const event = nativeDrop();
    const pending = deferred<{ created: string[]; failed: [] }>();
    const options = { rootPath: "/copy-root", isDir: () => true, onCopied: vi.fn(async () => true), onCreated: vi.fn() };
    h.invoke.mockImplementationOnce(() => pending.promise);
    h.render(() => useExplorerFileDrop(options));
    h.dragListen.mock.calls[0][0](event);
    h.render(() => useExplorerFileDrop({ ...options, rootPath: "/other" }));
    h.render(() => useExplorerFileDrop(options));
    pending.resolve({ created: ["/copy-root/dest/a"], failed: [] });
    await settle();
    expect(options.onCopied).not.toHaveBeenCalled();
    expect(options.onCreated).not.toHaveBeenCalled();
  });

  it("does not refresh a new root when an old native copy completes", async () => {
    const event = nativeDrop();
    const pending = deferred<{ created: string[]; failed: [] }>();
    const onCopied = vi.fn();
    h.invoke.mockImplementationOnce(() => pending.promise);
    h.render(() => useExplorerFileDrop({ rootPath: "/copy-root", isDir: () => true, onCopied }));
    h.dragListen.mock.calls[0][0](event);
    h.render(() => useExplorerFileDrop({ rootPath: "/other-root", isDir: () => true, onCopied }));
    pending.resolve({ created: ["/copy-root/dest/a"], failed: [] });
    await settle();
    expect(onCopied).not.toHaveBeenCalled();
  });

  it("detaches browser upload handlers while hidden and restores paste once", async () => {
    vi.stubGlobal("Element", class {});
    const handlers = new Map<string, (event: unknown) => void>();
    const element = {
      addEventListener: vi.fn((name, handler) => handlers.set(name, handler)),
      removeEventListener: vi.fn((name) => handlers.delete(name)),
    };
    const options = {
      containerRef: { current: element as unknown as HTMLElement },
      rootPath: "/uploads", selectedPath: null, isDir: () => false, onUploaded: vi.fn(),
    };
    h.render(() => useExplorerUpload({ ...options, enabled: false }));
    expect(element.addEventListener).not.toHaveBeenCalled();
    h.render(() => useExplorerUpload(options));
    const stalePaste = handlers.get("paste");
    expect(handlers.size).toBe(4);
    h.render(() => useExplorerUpload({ ...options, enabled: false }));
    expect(handlers.size).toBe(0);
    stalePaste?.({});
    expect(h.filesFrom).not.toHaveBeenCalled();
    h.filesFrom.mockReturnValue([{ name: "a.txt" }]);
    h.uploadFiles.mockResolvedValue([{ name: "a.txt" }]);
    h.render(() => useExplorerUpload(options));
    handlers.get("paste")?.({ target: null, preventDefault: vi.fn() });
    await settle();
    expect(h.uploadFiles).toHaveBeenCalledExactlyOnceWith("/uploads", [{ name: "a.txt" }], h.workspace);
    expect(options.onUploaded).toHaveBeenCalledExactlyOnceWith("/uploads");
  });

  it("releases a late native registration and ignores its stale events", async () => {
    const pending = deferred<() => void>();
    const stop = vi.fn();
    const options = { rootPath: "/drops", isDir: () => true, onCopied: vi.fn() };
    h.render(() => useExplorerFileDrop({ ...options, enabled: false }));
    expect(h.dragListen).not.toHaveBeenCalled();
    h.dragListen.mockImplementationOnce(() => pending.promise);
    h.render(() => useExplorerFileDrop(options));
    const staleHandler = h.dragListen.mock.calls[0][0];
    h.render(() => useExplorerFileDrop({ ...options, enabled: false }));
    pending.resolve(stop);
    await settle();
    staleHandler({ payload: { type: "drop", paths: ["/a"] } });
    expect(stop).toHaveBeenCalledOnce();
    expect(h.invoke).not.toHaveBeenCalled();
  });
});
