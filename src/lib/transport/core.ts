/**
 * Drop-in replacement for `@tauri-apps/api/core`, wired up by a Vite alias in
 * `vite.config.ts`. Signatures match Tauri's so no business module needs to
 * change — that is the whole point of this file.
 */

import { Channel, transport } from "./client";

export { Channel };

/**
 * `pty_write` is special: Terax passes raw bytes as the payload and the PTY id
 * in an `x-pty-id` header, skipping JSON on the per-keystroke path. We keep
 * that property by sending a binary frame instead — see `protocol.ts`.
 */
export async function invoke<T = unknown>(
  cmd: string,
  args?: unknown,
  options?: { headers?: Record<string, string> },
): Promise<T> {
  if (cmd === "pty_write") {
    const id = Number(options?.headers?.["x-pty-id"]);
    if (!Number.isFinite(id)) {
      throw new Error("pty_write: missing or invalid x-pty-id header");
    }
    const bytes =
      args instanceof Uint8Array
        ? args
        : args instanceof ArrayBuffer
          ? new Uint8Array(args)
          : null;
    if (!bytes) throw new Error("pty_write: expected raw bytes");
    await transport.ptyWrite(id, bytes);
    return undefined as T;
  }

  // The desktop build spawns a native window pointed at `settings.html`, which
  // Vite already emits as its own entry. A browser tab is the honest analogue,
  // so this stays in the shim rather than becoming a modal: the settings page
  // is a whole second app, and re-hosting it inside the main one would mean
  // maintaining a layout the desktop build never exercises.
  if (cmd === "open_settings_window") {
    const tab = (args as { tab?: string | null } | undefined)?.tab;
    const url = tab ? `/settings.html?tab=${encodeURIComponent(tab)}` : "/settings.html";
    // Named, so asking for settings twice focuses the existing tab instead of
    // piling up duplicates — the same thing the desktop window manager does.
    window.open(url, "phantom-settings");
    return undefined as T;
  }

  return transport.invoke<T>(cmd, (args ?? {}) as Record<string, unknown>);
}

/**
 * Tauri rewrites local file paths to an asset protocol URL. The server serves
 * workspace files over plain HTTP instead: `/files`, asked to serve inline.
 */
export function convertFileSrc(filePath: string, _protocol = "asset"): string {
  return `/files?${new URLSearchParams({ path: filePath, inline: "true" })}`;
}

/**
 * Everything below exists because the Vite alias catches `@tauri-apps/api/core`
 * imports inside the `@tauri-apps/plugin-*` packages too, not just Terax's own
 * call sites. Those plugins are being replaced or dropped for Phantom (they
 * wrap desktop-only capabilities), but their modules still have to link, so
 * these are honest stubs rather than working implementations.
 */

/** Tauri's handle to a server-side object. Nothing server-side allocates one. */
export class Resource {
  readonly rid: number;

  constructor(rid: number) {
    this.rid = rid;
  }

  async close(): Promise<void> {}
}

export class PluginListener {
  constructor(
    readonly plugin: string,
    readonly event: string,
    readonly channelId: number,
  ) {}

  async unregister(): Promise<void> {}
}

/**
 * Plugin event streams are a desktop-only mechanism (notifications, updater
 * progress). Returns a listener that never fires rather than throwing, so a
 * plugin's module-level setup doesn't break the page.
 */
export async function addPluginListener<T>(
  plugin: string,
  event: string,
  _cb: (payload: T) => void,
): Promise<PluginListener> {
  console.debug(`phantom: ignoring plugin listener ${plugin}:${event}`);
  return new PluginListener(plugin, event, 0);
}

/** Phantom is a web app, never a Tauri webview. Callers branch on this. */
export function isTauri(): boolean {
  return false;
}
