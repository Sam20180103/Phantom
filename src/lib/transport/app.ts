/**
 * Browser-safe stand-in for `@tauri-apps/api/app`. Values come from the build
 * rather than from a Tauri runtime that isn't there.
 */

export async function getName(): Promise<string> {
  return "Phantom";
}

export async function getVersion(): Promise<string> {
  return "0.1.0";
}

export async function getTauriVersion(): Promise<string> {
  return "0.0.0";
}
