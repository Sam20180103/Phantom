import { useCallback, useLayoutEffect, useRef } from "react";
import { pasteClipboardFiles } from "@/modules/explorer/lib/clipboardPaste";
import { useWorkspaceEnvStore, type WorkspaceEnv } from "@/modules/workspace";

type PasteScope = {
  root: string | null;
  workspace: WorkspaceEnv;
  enabled: boolean;
};

export function useExplorerClipboard(options: {
  rootPath: string | null;
  enabled: boolean;
  refresh: (path: string) => Promise<boolean>;
  onCreated?: (paths: string[]) => void;
}) {
  const workspace = useWorkspaceEnvStore((s) => s.env);
  const scopeRef = useRef<PasteScope | null>(null);
  const refreshRef = useRef(options.refresh);
  const onCreatedRef = useRef(options.onCreated);
  const busy = useRef(false);
  useLayoutEffect(() => {
    refreshRef.current = options.refresh;
    onCreatedRef.current = options.onCreated;
  }, [options.refresh, options.onCreated]);
  useLayoutEffect(() => {
    scopeRef.current = {
      root: options.rootPath,
      workspace,
      enabled: options.enabled,
    };
    return () => {
      scopeRef.current = null;
    };
  }, [options.rootPath, options.enabled, workspace]);

  return useCallback(async (destDir: string) => {
    const scope = scopeRef.current;
    if (!scope?.root || !scope.enabled || busy.current) return [];
    busy.current = true;
    try {
      const created = await pasteClipboardFiles(destDir, {
        root: scope.root,
        workspace: scope.workspace,
        isCurrent: () => scopeRef.current === scope,
        refresh: (path) => refreshRef.current(path),
      });
      if (scopeRef.current === scope && created.length > 0) {
        onCreatedRef.current?.(created);
      }
      return created;
    } finally {
      busy.current = false;
    }
  }, []);
}
