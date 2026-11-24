/**
 * Browser-safe stand-in for `@tauri-apps/api/window`, wired up by a Vite alias.
 *
 * Terax calls `getCurrentWindow()` during render (see `WindowControls.tsx`) and
 * inside mount effects. The real module reaches for `__TAURI_INTERNALS__`,
 * which does not exist in a browser, so it throws — and an exception during
 * render makes React unmount the entire tree, leaving a blank page. Hence
 * stubs rather than letting those imports resolve to the real thing.
 *
 * There is no OS window to control here: the browser owns minimize, maximize,
 * and close. These methods therefore no-op rather than pretend to work.
 */

export type UnlistenFn = () => void;

export type PhysicalSize = { width: number; height: number };

class BrowserWindowStub {
  readonly label = "main";

  async isMaximized(): Promise<boolean> {
    return false;
  }
  async isMinimized(): Promise<boolean> {
    return false;
  }
  async isFocused(): Promise<boolean> {
    return document.hasFocus();
  }
  async isVisible(): Promise<boolean> {
    return document.visibilityState === "visible";
  }

  async show(): Promise<void> {}
  async hide(): Promise<void> {}
  async minimize(): Promise<void> {}
  async maximize(): Promise<void> {}
  async unmaximize(): Promise<void> {}
  async toggleMaximize(): Promise<void> {}
  async setFocus(): Promise<void> {}
  async startDragging(): Promise<void> {}
  async setTitle(title: string): Promise<void> {
    document.title = title;
  }

  /** The browser tab is closed by the user, not by us. */
  async close(): Promise<void> {}
  async destroy(): Promise<void> {}

  async innerSize(): Promise<PhysicalSize> {
    return { width: window.innerWidth, height: window.innerHeight };
  }

  async onResized(handler: () => void): Promise<UnlistenFn> {
    window.addEventListener("resize", handler);
    return () => window.removeEventListener("resize", handler);
  }

  async onFocusChanged(
    handler: (event: { payload: boolean }) => void,
  ): Promise<UnlistenFn> {
    const onFocus = () => handler({ payload: true });
    const onBlur = () => handler({ payload: false });
    window.addEventListener("focus", onFocus);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("focus", onFocus);
      window.removeEventListener("blur", onBlur);
    };
  }

  /**
   * A browser tab cannot veto its own close, so the handler is never invoked.
   * Anything that must survive a close needs to be persisted eagerly instead
   * of in a close guard.
   */
  async onCloseRequested(
    _handler: (event: { preventDefault: () => void }) => void,
  ): Promise<UnlistenFn> {
    return () => {};
  }

  async listen(_event: string, _handler: (e: unknown) => void): Promise<UnlistenFn> {
    return () => {};
  }
}

const current = new BrowserWindowStub();

export function getCurrentWindow(): BrowserWindowStub {
  return current;
}

/** Alias kept for older call sites. */
export const appWindow = current;

export type Window = BrowserWindowStub;
