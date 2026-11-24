/**
 * Browser-safe stand-in for `@tauri-apps/api/webviewWindow`.
 *
 * A separate entry point from `window` and `webview`, and it fails harder: the
 * real `getCurrentWebviewWindow()` reads `__TAURI_INTERNALS__.metadata` at call
 * time, which throws `Cannot read properties of undefined` in a browser.
 *
 * Terax uses it only for `listen` (file-watch events, which now arrive over the
 * WebSocket) and `setFocus`.
 */

import { transport } from "./client";
import type { Event, UnlistenFn } from "./event";

class BrowserWebviewWindowStub {
  readonly label = "main";

  /** Server-originated events reach us on the socket, not through Tauri. */
  async listen<T>(
    event: string,
    handler: (event: Event<T>) => void,
  ): Promise<UnlistenFn> {
    return transport.listen(event, (payload) => {
      handler({ event, id: 0, payload: payload as T });
    });
  }

  async setFocus(): Promise<void> {
    window.focus();
  }

  async setTitle(title: string): Promise<void> {
    document.title = title;
  }
}

const current = new BrowserWebviewWindowStub();

export function getCurrentWebviewWindow(): BrowserWebviewWindowStub {
  return current;
}

export type WebviewWindow = BrowserWebviewWindowStub;
