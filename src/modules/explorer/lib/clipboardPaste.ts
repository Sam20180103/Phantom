import { invoke, isTauri } from "@tauri-apps/api/core";
import { toast } from "sonner";
import {
  copyFailureNotice,
  type CopyResult,
} from "@/modules/explorer/lib/copyResult";
import type { WorkspaceEnv } from "@/modules/workspace";

function normalize(path: string): string {
  const value = path.replace(/\\/g, "/");
  return value === "/" || /^[a-z]:\/$/i.test(value)
    ? value
    : value.replace(/\/+$/, "");
}

export function parentDir(path: string, fallback: string): string {
  const value = normalize(path);
  const i = value.lastIndexOf("/");
  if (i === 0) return "/";
  if (i === 2 && /^[a-z]:\//i.test(value)) return value.slice(0, 3);
  return i > 0 ? value.slice(0, i) : fallback;
}

export function pasteTargetDir(
  selected: string | null,
  isDir: (path: string) => boolean | undefined,
  rootPath: string,
): string {
  const root = normalize(rootPath);
  if (!selected) return root;
  const path = normalize(selected);
  if (path === root || path.split("/").includes("..")) return root;
  if (path !== root && !path.startsWith(root.endsWith("/") ? root : `${root}/`))
    return root;
  return isDir(path) ? path : parentDir(path, root);
}

export function isPasteShortcut(
  e: Pick<KeyboardEvent, "key" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey">,
  isMac: boolean,
): boolean {
  if (e.key.toLowerCase() !== "v" || e.altKey || e.shiftKey) return false;
  return isMac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
}

export function isEditablePasteTarget(target: EventTarget | null): boolean {
  const element = target as HTMLElement | null;
  return (
    element?.tagName === "INPUT" ||
    element?.tagName === "TEXTAREA" ||
    !!element?.isContentEditable
  );
}

export async function pasteClipboardFiles(
  destDir: string,
  options: {
    root: string;
    workspace: WorkspaceEnv;
    isCurrent: () => boolean;
    refresh: (path: string) => Promise<boolean>;
  },
): Promise<string[]> {
  if (!isTauri() || !options.isCurrent()) return [];
  let copyStarted = false;
  let refreshed = false;
  try {
    const sources = await invoke<string[]>("fs_clipboard_paths");
    if (!options.isCurrent()) return [];
    if (sources.length === 0) {
      toast.message("No files on the clipboard");
      return [];
    }
    copyStarted = true;
    const result = await invoke<CopyResult>("fs_copy", {
      sources,
      destDir,
      root: options.root,
      workspace: options.workspace,
      renameOnConflict: true,
    });
    const notice = copyFailureNotice(result);
    if (notice)
      toast.error(notice.message, { description: notice.description });
    if (!options.isCurrent() || result.created.length === 0) return [];
    refreshed = true;
    return (await options.refresh(destDir)) && options.isCurrent()
      ? result.created
      : [];
  } catch (error) {
    toast.error(
      refreshed
        ? `Files copied, but directory refresh failed: ${String(error)}`
        : `Paste failed: ${String(error)}`,
    );
    if (copyStarted && !refreshed && options.isCurrent()) {
      try {
        await options.refresh(destDir);
      } catch {
        /* The copy error is already reported. */
      }
    }
    return [];
  }
}
