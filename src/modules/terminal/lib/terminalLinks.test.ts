import { afterEach, describe, expect, it, vi } from "vitest";

const { openUrl, platform } = vi.hoisted(() => ({
  openUrl: vi.fn(),
  platform: { native: true },
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl }));
vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => platform.native }));

import { createTerminalLinkHandler } from "./terminalLinks";

describe("createTerminalLinkHandler", () => {
  afterEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
    platform.native = true;
  });

  it("opens OSC 8 links natively and restores late-bound terminal focus", async () => {
    openUrl.mockResolvedValue(undefined);
    const initialFocus = vi.fn();
    let focus = initialFocus;
    const handler = createTerminalLinkHandler(() => focus());
    focus = vi.fn();

    handler.activate(
      {} as MouseEvent,
      "https://chatgpt.com/codex/settings/usage",
    );

    expect(openUrl).toHaveBeenCalledWith(
      "https://chatgpt.com/codex/settings/usage",
    );
    await vi.waitFor(() => expect(focus).toHaveBeenCalledOnce());
    expect(initialFocus).not.toHaveBeenCalled();
  });

  it.each([
    "file:///etc/passwd",
    "javascript:alert(1)",
    "data:text/html,<script>alert(1)</script>",
  ])("never opens an unsafe OSC 8 target %s", async (uri) => {
    const focus = vi.fn();
    createTerminalLinkHandler(focus).activate({} as MouseEvent, uri);
    expect(openUrl).not.toHaveBeenCalled();
    expect(focus).toHaveBeenCalledOnce();
  });

  it("opens browser links without opener access and restores terminal focus", () => {
    platform.native = false;
    const open = vi.fn();
    vi.stubGlobal("window", { open });
    const focus = vi.fn();
    createTerminalLinkHandler(focus).activate(
      {} as MouseEvent,
      "https://example.com/中文",
    );
    expect(open).toHaveBeenCalledExactlyOnceWith(
      "https://example.com/中文",
      "_blank",
      "noopener,noreferrer",
    );
    expect(openUrl).not.toHaveBeenCalled();
    expect(focus).toHaveBeenCalledOnce();
  });

  it("restores terminal focus even when the native opener rejects", async () => {
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      openUrl.mockRejectedValueOnce(new Error("opener unavailable"));
      const focus = vi.fn();
      createTerminalLinkHandler(focus).activate(
        {} as MouseEvent,
        "https://example.com/",
      );
      await vi.waitFor(() => expect(focus).toHaveBeenCalledOnce());
    } finally {
      log.mockRestore();
    }
  });
});
