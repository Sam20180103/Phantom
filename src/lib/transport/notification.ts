/**
 * Browser-safe stand-in for `@tauri-apps/plugin-notification`, wired up by a
 * Vite alias.
 *
 * Deliberately minimal: a plain `Notification`, which shows while the page is
 * alive — typically "I switched to another tab and want to know when the agent
 * finishes". Terax's own in-app toast and bell already cover the case where
 * you're looking at the app.
 *
 * Not attempted: notifications that survive the page being closed. That needs
 * a service worker plus Web Push (VAPID keys, a subscription store, and the
 * server reaching a public push service), which is a lot of moving parts for
 * something Phantom doesn't need.
 *
 * Requires a secure context — over plain HTTP the Notification API is absent
 * and every call here degrades to a warning.
 */

export type Permission = "granted" | "denied" | "prompt";

function supported(): boolean {
  return typeof Notification !== "undefined";
}

/**
 * Asks for permission on the first real interaction.
 *
 * Browsers only show the prompt from a user gesture. Agent signals arrive
 * asynchronously, so requesting at the moment we want to notify silently
 * resolves to "default" and nothing appears — permission has to be secured
 * ahead of time, on a click or keypress the user made anyway.
 */
export function requestNotificationPermissionOnFirstGesture(): void {
  if (!supported() || Notification.permission !== "default") return;

  const events = ["pointerdown", "keydown"] as const;
  const ask = () => {
    void Notification.requestPermission().finally(() => {
      for (const event of events) window.removeEventListener(event, ask);
    });
  };
  for (const event of events) {
    window.addEventListener(event, ask, { once: true, passive: true });
  }
}

export async function isPermissionGranted(): Promise<boolean> {
  return supported() && Notification.permission === "granted";
}

export async function requestPermission(): Promise<Permission> {
  if (!supported()) {
    console.warn(
      "phantom: the Notification API is unavailable — this needs a secure " +
        "context (HTTPS or localhost).",
    );
    return "denied";
  }
  const result = await Notification.requestPermission();
  return result === "default" ? "prompt" : result;
}

export type NotificationOptions = {
  title: string;
  body?: string;
  /** Later notifications with the same tag replace earlier ones. */
  tag?: string;
  icon?: string;
  silent?: boolean;
};

export function sendNotification(options: NotificationOptions | string): void {
  const opts = typeof options === "string" ? { title: options } : options;
  if (!supported() || Notification.permission !== "granted") return;

  try {
    new Notification(opts.title, {
      body: opts.body,
      icon: opts.icon ?? "/logo.png",
      // Collapse repeats rather than stacking one per agent turn.
      tag: opts.tag ?? "phantom-agent",
      silent: opts.silent,
    });
  } catch (e) {
    console.warn("phantom: notification failed:", e);
  }
}

/** Part of the plugin's surface; nothing calls these yet. */
export async function registerActionTypes(): Promise<void> {}
export async function cancel(): Promise<void> {}
export async function removeActive(): Promise<void> {}
export async function active(): Promise<never[]> {
  return [];
}
