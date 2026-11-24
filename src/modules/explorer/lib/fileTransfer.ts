/**
 * Moving whole files between the browser and the workspace.
 *
 * Plain HTTP rather than the WebSocket: the bytes would otherwise be base64'd
 * through a JSON frame, and a download would lose the browser's own save-file
 * dialog. The endpoints are same-origin, so the session cookie rides along.
 */

import { currentWorkspaceEnv, type WorkspaceEnv } from "@/modules/workspace";

type UploadResult = { path: string; name: string };

function withWorkspace(params: URLSearchParams, env: WorkspaceEnv = currentWorkspaceEnv()): URLSearchParams {
  if (env.kind === "wsl") params.set("distro", env.distro);
  return params;
}

export function downloadUrl(path: string): string {
  return `/files?${withWorkspace(new URLSearchParams({ path }))}`;
}

/**
 * Hands a workspace file to the browser as a download.
 *
 * A temporary anchor rather than `location.href`: the click keeps the SPA on
 * the page, and `Content-Disposition: attachment` from the server is what
 * actually opens the save dialog.
 */
export function downloadFile(path: string): void {
  const a = document.createElement("a");
  a.href = downloadUrl(path);
  // No name of our own — the server sends the real one, including the
  // suffix it may have added on upload.
  a.download = "";
  a.rel = "noopener";
  document.body.appendChild(a);
  a.click();
  a.remove();
}

/** Uploads one file into `destDir`. Resolves to where it actually landed. */
export async function uploadFile(
  destDir: string,
  file: File,
  workspace: WorkspaceEnv = currentWorkspaceEnv(),
): Promise<UploadResult> {
  const params = withWorkspace(
    new URLSearchParams({ dir: destDir, name: file.name }), workspace,
  );
  const response = await fetch(`/files?${params}`, {
    method: "POST",
    body: file,
  });
  if (!response.ok) {
    throw new Error(`${response.status} ${await response.text()}`);
  }
  return (await response.json()) as UploadResult;
}

/**
 * Uploads sequentially, not in parallel: the server names each file only when
 * it opens it, so two uploads of the same name racing would both see a free
 * slot and one would take a suffix it didn't need — or worse, collide.
 */
export async function uploadFiles(
  destDir: string,
  files: File[],
  workspace: WorkspaceEnv = currentWorkspaceEnv(),
): Promise<UploadResult[]> {
  const out: UploadResult[] = [];
  for (const file of files) out.push(await uploadFile(destDir, file, workspace));
  return out;
}

/**
 * Opens the OS file picker, resolving with what was chosen — empty on cancel,
 * where the browser reports one.
 */
export function pickFiles(): Promise<File[]> {
  return new Promise((resolve) => {
    const input = document.createElement("input");
    input.type = "file";
    input.multiple = true;
    input.style.display = "none";
    const settle = (files: File[]) => {
      input.remove();
      resolve(files);
    };
    input.addEventListener(
      "change",
      () => settle(Array.from(input.files ?? [])),
      { once: true },
    );
    input.addEventListener("cancel", () => settle([]), { once: true });
    document.body.appendChild(input);
    input.click();
  });
}

/** Pulls files out of a clipboard or drag payload. */
export function filesFrom(data: DataTransfer | null): File[] {
  if (!data) return [];
  const out: File[] = [];
  for (const item of data.items) {
    if (item.kind !== "file") continue;
    const file = item.getAsFile();
    if (file) out.push(file);
  }
  return out;
}
