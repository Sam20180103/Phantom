import { invoke } from "@tauri-apps/api/core";
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { toast } from "sonner";
import { batchRootPaths, planBatchMove } from "@/modules/explorer/lib/batchPaths";
import { replaceMove } from "@/modules/explorer/lib/replaceMove";
import {
  currentWorkspaceEnv,
  useWorkspaceEnvStore,
  workspaceScopeKey,
} from "@/modules/workspace";
import { createWatchRegistry } from "@/modules/explorer/lib/watchRegistry";
import { createTreeReadScope } from "@/modules/explorer/lib/treeReadScope";
import { usePreferencesStore } from "@/modules/settings/preferences";
import { listenFsChanged, watchAdd, watchRemove } from "./watch";

export type DirEntry = {
  name: string;
  kind: "file" | "dir" | "symlink";
  size: number;
  mtime: number;
  gitignored: boolean;
};

type ChildrenState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; entries: DirEntry[] }
  | { status: "error"; message: string };

type TreeState = Record<string, ChildrenState>;

export type PendingCreate = {
  parentPath: string;
  kind: "file" | "dir";
};

export function joinPath(parent: string, name: string): string {
  if (parent.endsWith("/")) return `${parent}${name}`;
  return `${parent}/${name}`;
}

export function dirname(path: string): string {
  const i = path.lastIndexOf("/");
  if (i <= 0) return "/";
  return path.slice(0, i);
}

const EXPANSION_CACHE_LIMIT = 8;
const expansionCache = new Map<string, string[]>();

function rememberExpansion(root: string, expanded: Set<string>): void {
  expansionCache.delete(root);
  if (expanded.size > 0) expansionCache.set(root, [...expanded]);
  while (expansionCache.size > EXPANSION_CACHE_LIMIT) {
    const oldest = expansionCache.keys().next().value;
    if (oldest === undefined) break;
    expansionCache.delete(oldest);
  }
}

function recallExpansion(root: string): string[] {
  const v = expansionCache.get(root);
  if (!v) return [];
  expansionCache.delete(root);
  expansionCache.set(root, v);
  return v;
}

function isUnder(key: string, root: string): boolean {
  return key === root || key.startsWith(`${root}/`);
}

// mtime/size are ignored on purpose: the tree never renders them, so a watcher
// refetch that only bumps mtime (saving a file) must not count as a change.
function sameDirListing(a: DirEntry[], b: DirEntry[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    if (
      a[i].name !== b[i].name ||
      a[i].kind !== b[i].kind ||
      a[i].gitignored !== b[i].gitignored
    )
      return false;
  }
  return true;
}

type Options = {
  onPathRenamed?: (from: string, to: string) => void;
  onPathDeleted?: (path: string) => void;
  active?: boolean;
};

export function useFileTree(rootPath: string | null, options?: Options) {
  const workspace = useWorkspaceEnvStore((s) => s.env);
  const cacheKey = `${workspaceScopeKey(workspace)}\u0000${rootPath ?? ""}`;
  const active = options?.active ?? true;
  // biome-ignore lint/correctness/useExhaustiveDependencies: Each root/environment needs an independent request lifetime.
  const readScope = useMemo(() => createTreeReadScope(), [rootPath, workspace]);
  const watches = useMemo(
    () => createWatchRegistry({
      add: (paths) => watchAdd(paths, [], workspace),
      remove: (paths) => watchRemove(paths, [], workspace),
    }),
    [workspace],
  );
  const showHidden = usePreferencesStore((s) => s.showHidden);
  const gitDecorations = usePreferencesStore((s) => s.explorerGitDecorations);
  const [nodes, setNodes] = useState<TreeState>({});
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [pendingCreate, setPendingCreate] = useState<PendingCreate | null>(
    null,
  );
  const [renaming, setRenaming] = useState<string | null>(null);

  const expandedRef = useRef(expanded);
  const nodesRef = useRef(nodes);

  useLayoutEffect(() => {
    if (active && rootPath) readScope.activate();
    else readScope.suspend();
    return () => {
      readScope.suspend();
      watches.disarm();
    };
  }, [active, rootPath, readScope, watches]);

  useEffect(() => {
    expandedRef.current = expanded;
  }, [expanded]);

  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  const addWatch = useCallback((path: string) => watches.add(path), [watches]);
  const removeWatch = useCallback(
    (path: string) => watches.remove(path),
    [watches],
  );

  const fetchChildren = useCallback(async (path: string) => {
    const request = readScope.begin(path);
    if (!request) return false;
    if (nodesRef.current[path]?.status !== "loaded") {
      setNodes((s) => ({ ...s, [path]: { status: "loading" } }));
    }
    try {
      const entries = await invoke<DirEntry[]>("fs_read_dir", {
        path,
        showHidden,
        gitDecorations,
        workspace,
      });
      if (!request.isCurrent()) return false;

      const prev = nodesRef.current[path];
      if (prev?.status === "loaded" && sameDirListing(prev.entries, entries)) {
        return true;
      }

      const liveDirs = new Set(
        entries.filter((e) => e.kind === "dir").map((e) => joinPath(path, e.name)),
      );
      const removedRoots: string[] = [];
      for (const key of Object.keys(nodesRef.current)) {
        if (dirname(key) === path && !liveDirs.has(key)) removedRoots.push(key);
      }
      const dead = new Set<string>();
      if (removedRoots.length > 0) {
        readScope.cancelUnder(removedRoots);
        const candidates = new Set<string>([
          ...Object.keys(nodesRef.current),
          ...expandedRef.current,
          ...watches.paths(),
        ]);
        for (const k of candidates) {
          if (removedRoots.some((r) => isUnder(k, r))) dead.add(k);
        }
      }

      setNodes((s) => {
        const next: TreeState = {};
        for (const [k, v] of Object.entries(s)) if (!dead.has(k)) next[k] = v;
        next[path] = { status: "loaded", entries };
        return next;
      });

      if (dead.size > 0) {
        setExpanded((c) => {
          let changed = false;
          const n = new Set(c);
          for (const d of dead) if (n.delete(d)) changed = true;
          return changed ? n : c;
        });
        watches.removeMany([...dead]);
      }
      return true;
    } catch (e) {
      if (!request.isCurrent()) return false;
      setNodes((s) => ({
        ...s,
        [path]: { status: "error", message: String(e) },
      }));
      return false;
    } finally {
      request.finish();
    }
  }, [readScope, workspace, watches, showHidden, gitDecorations]);

  // Root change → restore the cached expansion for this root, re-scope watches,
  // and persist the outgoing root's expansion on the way out.
  useEffect(() => {
    if (!rootPath) {
      nodesRef.current = {};
      expandedRef.current = new Set();
      setNodes({});
      setExpanded(new Set());
      setPendingCreate(null);
      setRenaming(null);
      return;
    }
    setPendingCreate(null);
    setRenaming(null);

    const restored = recallExpansion(cacheKey);
    const expansion = new Set(restored);
    expandedRef.current = expansion;
    setExpanded(expansion);
    setNodes({});
    // Reads can resolve before the cleared state commits, so drop the old cache now.
    nodesRef.current = {};

    watches.addMany([rootPath, ...restored]);

    return () => {
      rememberExpansion(cacheKey, expandedRef.current);
      watches.clear();
    };
  }, [rootPath, cacheKey, watches]);

  useEffect(() => {
    if (!active || !rootPath) {
      watches.disarm();
      return;
    }
    watches.arm();
    const targets = new Set(watches.paths());
    for (const [path, state] of Object.entries(nodesRef.current)) {
      if (state.status === "loaded") targets.add(path);
    }
    for (const path of targets) void fetchChildren(path);
  }, [active, rootPath, watches, fetchChildren]);

  useEffect(() => {
    if (!active || !rootPath) return;
    let alive = true;
    let unlisten: (() => void) | undefined;
    void listenFsChanged((paths) => {
      if (!alive) return;
      const current = nodesRef.current;
      const dirs = new Set<string>();
      for (const p of paths) {
        const parent = dirname(p);
        if (current[parent]?.status === "loaded") dirs.add(parent);
        if (current[p]?.status === "loaded") dirs.add(p);
      }
      for (const d of dirs) void fetchChildren(d);
    }).then((un) => {
      if (alive) unlisten = un;
      else un();
    });
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [active, rootPath, fetchChildren]);

  const toggle = useCallback(
    (path: string) => {
      if (expandedRef.current.has(path)) {
        setExpanded((curr) => {
          const next = new Set(curr);
          next.delete(path);
          return next;
        });
        removeWatch(path);
      } else {
        setExpanded((curr) => {
          const next = new Set(curr);
          next.add(path);
          return next;
        });
        addWatch(path);
        void fetchChildren(path);
      }
    },
    [fetchChildren, addWatch, removeWatch],
  );

  const expand = useCallback(
    (path: string) => {
      if (expandedRef.current.has(path)) return;
      setExpanded((curr) => {
        const next = new Set(curr);
        next.add(path);
        return next;
      });
      addWatch(path);
      void fetchChildren(path);
    },
    [fetchChildren, addWatch],
  );

  const refresh = useCallback(
    async (path: string, reveal = false) => {
      if (!readScope.isActive()) return false;
      if (reveal && path !== rootPath) {
        setExpanded((current) => new Set(current).add(path));
        addWatch(path);
      }
      return fetchChildren(path);
    },
    [fetchChildren, readScope, rootPath, addWatch],
  );

  // --- mutations ---

  const beginCreate = useCallback(
    (parentPath: string, kind: "file" | "dir") => {
      setRenaming(null);
      setPendingCreate({ parentPath, kind });
      // Ensure the parent is expanded so the input row is visible.
      if (rootPath && parentPath !== rootPath) {
        setExpanded((curr) => {
          if (curr.has(parentPath)) return curr;
          const next = new Set(curr);
          next.add(parentPath);
          return next;
        });
        addWatch(parentPath);
      }
      setNodes((curr) => {
        if (!curr[parentPath]) void fetchChildren(parentPath);
        return curr;
      });
    },
    [rootPath, fetchChildren, addWatch],
  );

  const cancelCreate = useCallback(() => setPendingCreate(null), []);

  const commitCreate = useCallback(
    async (name: string) => {
      if (!pendingCreate) return;
      const trimmed = name.trim();
      if (!trimmed) {
        setPendingCreate(null);
        return;
      }
      const path = joinPath(pendingCreate.parentPath, trimmed);
      const cmd =
        pendingCreate.kind === "dir" ? "fs_create_dir" : "fs_create_file";
      try {
        await invoke(cmd, { path, workspace: currentWorkspaceEnv() });
        await fetchChildren(pendingCreate.parentPath);
      } catch (e) {
        console.error(`${cmd} failed:`, e);
      } finally {
        setPendingCreate(null);
      }
    },
    [pendingCreate, fetchChildren],
  );

  const beginRename = useCallback((path: string) => {
    setPendingCreate(null);
    setRenaming(path);
  }, []);

  const cancelRename = useCallback(() => setRenaming(null), []);

  const commitRename = useCallback(
    async (newName: string) => {
      if (!renaming) return;
      const trimmed = newName.trim();
      const parent = dirname(renaming);
      const oldName = renaming.slice(parent === "/" ? 1 : parent.length + 1);
      if (!trimmed || trimmed === oldName) {
        setRenaming(null);
        return;
      }
      const to = joinPath(parent, trimmed);
      try {
        await invoke("fs_rename", {
          from: renaming,
          to,
          workspace: currentWorkspaceEnv(),
        });
        options?.onPathRenamed?.(renaming, to);
        await fetchChildren(parent);
      } catch (e) {
        console.error("fs_rename failed:", e);
      } finally {
        setRenaming(null);
      }
    },
    [renaming, fetchChildren, options],
  );

  const deletePath = useCallback(
    async (path: string) => {
      try {
        await invoke("fs_delete", { path, workspace: currentWorkspaceEnv() });
        options?.onPathDeleted?.(path);
        await fetchChildren(dirname(path));
      } catch (e) {
        console.error("fs_delete failed:", e);
      }
    },
    [fetchChildren, options],
  );

  const deletePaths = useCallback(
    async (selectedPaths: string[]) => {
      const paths = batchRootPaths(selectedPaths);
      if (paths.length === 0) return;
      const results = await Promise.allSettled(
        paths.map((path) =>
          invoke("fs_delete", { path, workspace: currentWorkspaceEnv() }),
        ),
      );
      const parents = new Set<string>();
      let anyFailed = false;
      paths.forEach((path, i) => {
        if (results[i].status === "fulfilled") {
          options?.onPathDeleted?.(path);
          parents.add(dirname(path));
        } else {
          anyFailed = true;
          console.error(
            "fs_delete failed:",
            (results[i] as PromiseRejectedResult).reason,
          );
        }
      });
      await Promise.all([...parents].map((p) => fetchChildren(p)));
      if (anyFailed) toast.error("Delete failed for one or more items");
    },
    [fetchChildren, options],
  );

  const movePath = useCallback(
    async (from: string, toDir: string) => {
      const name = from.slice(from.lastIndexOf("/") + 1);
      const to = joinPath(toDir, name);
      if (to === from) return;
      const target = nodesRef.current[toDir];
      if (
        target?.status === "loaded" &&
        target.entries.some((e) => e.name === name)
      ) {
        console.warn(`move skipped: "${name}" already exists in ${toDir}`);
        return;
      }
      try {
        await invoke("fs_rename", {
          from,
          to,
          workspace: currentWorkspaceEnv(),
        });
        options?.onPathRenamed?.(from, to);
        await Promise.all([fetchChildren(dirname(from)), fetchChildren(toDir)]);
      } catch (e) {
        console.error("fs_rename (move) failed:", e);
      }
    },
    [fetchChildren, options],
  );

  const movePaths = useCallback(
    async (sources: string[], toDir: string) => {
      if (sources.length === 0) return;

      const workspace = currentWorkspaceEnv();
      let existingNames: string[];
      try {
        const entries = await invoke<DirEntry[]>("fs_read_dir", {
          path: toDir,
          showHidden: true,
          gitDecorations: false,
          workspace,
        });
        existingNames = entries.map((entry) => entry.name);
      } catch (error) {
        console.error("fs_read_dir (move) failed:", error);
        toast.error("Could not read the destination directory");
        return;
      }
      const items = planBatchMove(sources, toDir, existingNames);
      if (items.length === 0) return;

      const parents = new Set<string>([toDir]);
      let anyUnexpectedFailure = false;

      const moveOne = async (from: string, to: string) => {
        await invoke("fs_rename", { from, to, workspace });
        options?.onPathRenamed?.(from, to);
        parents.add(dirname(from));
      };

      const nonConflicting = items.filter((i) => !i.conflict);
      const conflicting = items.filter((i) => i.conflict);

      const results = await Promise.allSettled(
        nonConflicting.map((i) => moveOne(i.from, i.to)),
      );
      for (const r of results) {
        if (r.status === "rejected") {
          anyUnexpectedFailure = true;
          console.error("fs_rename (move) failed:", r.reason);
        }
      }

      for (const item of conflicting) {
        const resolution = await new Promise<"replace" | "skip">((resolve) => {
          toast.warning(`"${item.name}" already exists`, {
            duration: Infinity,
            action: { label: "Replace", onClick: () => resolve("replace") },
            cancel: { label: "Skip", onClick: () => resolve("skip") },
            onDismiss: () => resolve("skip"),
          });
        });
        if (resolution === "skip") continue;
        try {
          const backup = joinPath(toDir, `.phantom-move-${crypto.randomUUID()}.bak`);
          const warning = await replaceMove(item.from, item.to, backup, {
            rename: (from, to) => invoke("fs_rename", { from, to, workspace }),
            remove: (path) => invoke("fs_delete", { path, workspace }),
          });
          options?.onPathRenamed?.(item.from, item.to);
          parents.add(dirname(item.from));
          if (warning) toast.warning(warning, { duration: Infinity });
        } catch (e) {
          anyUnexpectedFailure = true;
          console.error("fs_rename (replace) failed:", e);
          toast.error(String(e), { duration: Infinity });
        }
      }

      await Promise.all([...parents].map((p) => fetchChildren(p)));

      if (anyUnexpectedFailure) {
        toast.error("Move failed for one or more items");
      }
    },
    [fetchChildren, options],
  );

  return {
    nodes,
    expanded,
    pendingCreate,
    renaming,
    toggle,
    expand,
    refresh,
    beginCreate,
    cancelCreate,
    commitCreate,
    beginRename,
    cancelRename,
    commitRename,
    deletePath,
    deletePaths,
    movePath,
    movePaths,
    joinPath,
  };
}
