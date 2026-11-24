/**
 * Fills in the Web Crypto helpers that browsers withhold outside a **secure
 * context** (HTTPS, or a `localhost` origin).
 *
 * Phantom is normally reached at a private mesh address like
 * `http://192.0.2.5:8099`, which is *not* a secure context. The browser then
 * removes `crypto.randomUUID` and `crypto.subtle`, and code that calls them
 * throws — Terax uses `randomUUID` for session and message ids, so the AI
 * panel and settings stores fail to hydrate.
 *
 * ⚠️ **This is a stopgap for local development, not a fix.** It restores only
 * `randomUUID`. A plain-HTTP origin still loses:
 *
 *   * `crypto.subtle` — no Web Crypto
 *   * `navigator.clipboard` — breaks image paste (implementation plan §9)
 *   * the Notification and Push APIs — breaks agent-completion push (§6.3),
 *     which is Phantom's main reason to exist
 *   * Service Workers — breaks the PWA install and `share_target` path (§5.3)
 *
 * In other words, TLS is not a hardening step to do later: several planned
 * features cannot work without it. Serve Phantom over HTTPS (mesh CA or
 * Tailscale certificates), per §11.
 */

declare global {
  interface Crypto {
    randomUUID(): `${string}-${string}-${string}-${string}-${string}`;
  }
}

export function installSecureContextPolyfills(): void {
  if (window.isSecureContext) return;

  console.warn(
    "phantom: insecure context (plain HTTP on a non-localhost origin). " +
      "Clipboard, notifications, push, and service workers are unavailable — " +
      "serve over HTTPS to enable them.",
  );

  if (typeof crypto !== "undefined" && typeof crypto.randomUUID !== "function") {
    Object.defineProperty(crypto, "randomUUID", {
      configurable: true,
      writable: true,
      value: () => {
        // RFC 4122 v4 layout. `getRandomValues` stays available on insecure
        // origins, so this keeps real entropy — only the helper was removed.
        const bytes = new Uint8Array(16);
        crypto.getRandomValues(bytes);
        bytes[6] = (bytes[6] & 0x0f) | 0x40; // version 4
        bytes[8] = (bytes[8] & 0x3f) | 0x80; // variant 10xx
        const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0"));
        return [
          hex.slice(0, 4).join(""),
          hex.slice(4, 6).join(""),
          hex.slice(6, 8).join(""),
          hex.slice(8, 10).join(""),
          hex.slice(10, 16).join(""),
        ].join("-") as ReturnType<Crypto["randomUUID"]>;
      },
    });
  }
}
