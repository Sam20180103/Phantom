/**
 * Browser-safe stand-in for `@tauri-apps/api/path`.
 *
 * Path resolution belongs to the server — the browser has no filesystem. These
 * return POSIX-shaped placeholders so callers that only join and display paths
 * keep working; anything that needs a real host path must ask the server.
 */

export const sep = "/";

export async function join(...parts: string[]): Promise<string> {
  return parts
    .filter((p) => p.length > 0)
    .join("/")
    .replace(/\/{2,}/g, "/");
}

export async function homeDir(): Promise<string> {
  return "/";
}

export async function appConfigDir(): Promise<string> {
  return "/";
}

export async function appDataDir(): Promise<string> {
  return "/";
}

export async function dirname(path: string): Promise<string> {
  const i = path.replace(/\/+$/, "").lastIndexOf("/");
  return i <= 0 ? "/" : path.slice(0, i);
}

export async function basename(path: string): Promise<string> {
  return path.replace(/\/+$/, "").split("/").pop() ?? "";
}
