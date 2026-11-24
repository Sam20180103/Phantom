/**
 * Browser-safe stand-in for `@tauri-apps/api/webview`. See `window.ts` for why
 * these are stubs rather than the real module.
 *
 * Terax uses this only for native drag-and-drop of files onto the explorer and
 * terminal. The browser has its own HTML5 drag-and-drop, which those modules
 * can adopt later; until then the handler is simply never called.
 */

export type UnlistenFn = () => void;

class BrowserWebviewStub {
  readonly label = "main";

  /** Native file drops don't exist in a browser; wire HTML5 DnD instead. */
  async onDragDropEvent(_handler: (event: unknown) => void): Promise<UnlistenFn> {
    return () => {};
  }

  async listen(_event: string, _handler: (e: unknown) => void): Promise<UnlistenFn> {
    return () => {};
  }
}

const current = new BrowserWebviewStub();

export function getCurrentWebview(): BrowserWebviewStub {
  return current;
}

export type Webview = BrowserWebviewStub;
