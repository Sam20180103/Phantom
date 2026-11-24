import { currentWorkspaceEnv, type WorkspaceEnv } from "@/modules/workspace";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const FS_CHANGED_EVENT = "fs:changed";

type FsChangedPayload = { paths: string[] };

let pending: Promise<unknown> = Promise.resolve();

function updateWatch(
  command: string,
  paths: string[],
  files: string[],
  workspace: WorkspaceEnv,
): void {
  if (paths.length === 0 && files.length === 0) return;
  pending = pending
    .then(() => invoke(command, { paths, files, workspace }))
    .catch(() => {});
}

export function watchAdd(
  paths: string[],
  files: string[] = [],
  workspace = currentWorkspaceEnv(),
): void {
  updateWatch("fs_watch_add", paths, files, workspace);
}

export function watchRemove(
  paths: string[],
  files: string[] = [],
  workspace = currentWorkspaceEnv(),
): void {
  updateWatch("fs_watch_remove", paths, files, workspace);
}

export function listenFsChanged(
  handler: (paths: string[]) => void,
): Promise<() => void> {
  return getCurrentWebviewWindow().listen<FsChangedPayload>(
    FS_CHANGED_EVENT,
    (e) => handler(e.payload.paths),
  );
}

export function parentDir(path: string): string {
  const i = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  if (i <= 0) return path.slice(0, i + 1) || path;
  return path.slice(0, i);
}
