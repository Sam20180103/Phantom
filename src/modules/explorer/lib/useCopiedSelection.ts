import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useWorkspaceEnvStore } from "@/modules/workspace";

export function copiedSelection(created: string[], visible: string[]): string[] {
  const paths = new Set(created.map((path) => path.replace(/\\/g, "/")));
  return visible.filter((path) => paths.has(path));
}

export function useCopiedSelection(options: {
  rootPath: string | null;
  enabled: boolean;
  entryPaths: string[];
  onSelect: (paths: string[]) => void;
}) {
  const workspace = useWorkspaceEnvStore((s) => s.env);
  const scopeRef = useRef<object | null>(null);
  const [pending, setPending] = useState<{ scope: object; paths: string[] } | null>(null);

  useLayoutEffect(() => {
    scopeRef.current = options.enabled && options.rootPath
      ? { root: options.rootPath, workspace }
      : null;
    return () => { scopeRef.current = null; };
  }, [options.rootPath, options.enabled, workspace]);

  useEffect(() => {
    if (!pending) return;
    setPending(null);
    if (pending.scope !== scopeRef.current) return;
    const paths = copiedSelection(pending.paths, options.entryPaths);
    if (paths.length > 0) options.onSelect(paths);
  }, [pending, options.entryPaths, options.onSelect]);

  return useCallback((paths: string[]) => {
    const scope = scopeRef.current;
    if (scope && paths.length > 0) setPending({ scope, paths });
  }, []);
}
