import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { currentWorkspaceEnv, useWorkspaceEnvStore } from "@/modules/workspace";
import {
  copyFailureNotice,
  type CopyResult,
} from "@/modules/explorer/lib/copyResult";

type Options = {
  rootPath: string | null;
  enabled?: boolean;
  isDir: (path: string) => boolean | undefined;
  onCopied: (destDir: string) => Promise<boolean>;
  onCreated?: (paths: string[]) => void;
};

function parentDir(path: string): string {
  const i = path.lastIndexOf("/");
  return i > 0 ? path.slice(0, i) : path;
}

// Tauri reports the drop point in physical pixels on some platforms; scale down
// only when it overflows the logical viewport (mirrors the terminal drop).
function dirAt(
  x: number,
  y: number,
  rootPath: string | null,
  isDir: (p: string) => boolean | undefined,
): string | null {
  let lx = x;
  let ly = y;
  if (x > window.innerWidth || y > window.innerHeight) {
    const dpr = window.devicePixelRatio || 1;
    lx = x / dpr;
    ly = y / dpr;
  }
  const el = document.elementFromPoint(lx, ly) as HTMLElement | null;
  if (!el) return null;
  const row = el.closest<HTMLElement>("[data-fs-path]");
  if (row) {
    const p = row.getAttribute("data-fs-path") as string;
    return isDir(p) ? p : parentDir(p);
  }
  if (el.closest("[data-explorer-drop]")) return rootPath;
  return null;
}

// Accepts files dropped from the OS onto an explorer folder (copy, not move),
// via Tauri's native drag-drop. One webview-level listener; ignores drops that
// land outside the explorer (the terminal handles its own).
export function useExplorerFileDrop({
  rootPath,
  enabled = true,
  isDir,
  onCopied,
  onCreated,
}: Options) {
  const [targetDir, setTargetDir] = useState<string | null>(null);
  const optsRef = useRef({ rootPath, enabled, isDir, onCopied });
  optsRef.current = { rootPath, enabled, isDir, onCopied };
  const workspaceEnv = useWorkspaceEnvStore((s) => s.env);
  const scopeRef = useRef<object | null>(null);
  const onCreatedRef = useRef(onCreated);
  useLayoutEffect(() => {
    onCreatedRef.current = onCreated;
  }, [onCreated]);
  useLayoutEffect(() => {
    scopeRef.current = enabled && rootPath ? { rootPath, workspace: workspaceEnv } : null;
    return () => { scopeRef.current = null; };
  }, [rootPath, enabled, workspaceEnv]);

  useEffect(() => {
    if (!enabled) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;

    void getCurrentWebview()
      .onDragDropEvent((e) => {
        if (disposed || !optsRef.current.enabled) return;
        const p = e.payload;
        const { rootPath, isDir, onCopied } = optsRef.current;
        if (p.type === "enter" || p.type === "over") {
          setTargetDir(dirAt(p.position.x, p.position.y, rootPath, isDir));
          return;
        }
        if (p.type === "leave") {
          setTargetDir(null);
          return;
        }
        if (p.type === "drop") {
          const dir = dirAt(p.position.x, p.position.y, rootPath, isDir);
          setTargetDir(null);
          if (!rootPath || !dir || p.paths.length === 0) return;
          const workspace = currentWorkspaceEnv();
          const scope = scopeRef.current;
          const isCurrent = () => !disposed && optsRef.current.enabled &&
            scopeRef.current === scope && currentWorkspaceEnv() === workspace;
          void invoke<CopyResult>("fs_copy", {
            sources: p.paths,
            destDir: dir,
            root: rootPath,
            workspace,
          })
            .then(async (result) => {
              const notice = copyFailureNotice(result);
              if (notice) {
                toast.error(notice.message, { description: notice.description });
              }
              if (result.created.length === 0 || !isCurrent()) return;
              try {
                if (await onCopied(dir) && isCurrent()) {
                  onCreatedRef.current?.(result.created);
                }
              } catch (err) {
                toast.error(`Files copied, but directory refresh failed: ${String(err)}`);
              }
            })
            .catch((err) => toast.error(`Copy failed: ${String(err)}`));
        }
      })
      .then((fn) => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch((err) => console.error("[terax] explorer drop listen failed:", err));

    return () => {
      disposed = true;
      setTargetDir(null);
      unlisten?.();
    };
  }, [enabled]);

  return { externalTargetDir: targetDir };
}
