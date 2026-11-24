import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  isEditablePasteTarget,
  isPasteShortcut,
  parentDir,
  pasteClipboardFiles,
  pasteTargetDir,
} from "@/modules/explorer/lib/clipboardPaste";

const h = vi.hoisted(() => ({
  invoke: vi.fn(),
  error: vi.fn(),
  message: vi.fn(),
  native: true,
}));
vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
  isTauri: () => h.native,
}));
vi.mock("sonner", () => ({ toast: { error: h.error, message: h.message } }));

beforeEach(() => {
  vi.resetAllMocks();
  h.native = true;
});

describe("clipboard destinations and shortcuts", () => {
  it("handles filesystem roots and Windows separators", () => {
    expect(parentDir("/a", "/fallback")).toBe("/");
    expect(parentDir("C:\\a", "/fallback")).toBe("C:/");
    expect(parentDir("relative", "/fallback")).toBe("/fallback");
  });
  it("pastes inside a selected folder or beside a selected file", () => {
    expect(pasteTargetDir("/repo/src", () => true, "/repo")).toBe("/repo/src");
    expect(pasteTargetDir("/repo/src/a", () => false, "/repo")).toBe(
      "/repo/src",
    );
    expect(pasteTargetDir(null, () => false, "/repo/")).toBe("/repo");
    expect(pasteTargetDir("C:\\a", () => false, "C:\\")).toBe("C:/");
  });
  it("does not use a stale selection from another root or a prefix sibling", () => {
    expect(pasteTargetDir("/old/a", () => true, "/repo")).toBe("/repo");
    expect(pasteTargetDir("/repo-other/a", () => true, "/repo")).toBe("/repo");
    expect(pasteTargetDir("/repo", () => false, "/repo")).toBe("/repo");
    expect(pasteTargetDir("/repo/../outside", () => true, "/repo")).toBe(
      "/repo",
    );
  });
  it("uses only the platform's plain paste shortcut", () => {
    const key = {
      key: "V",
      metaKey: false,
      ctrlKey: true,
      shiftKey: false,
      altKey: false,
    };
    expect(isPasteShortcut(key, false)).toBe(true);
    expect(isPasteShortcut(key, true)).toBe(false);
    expect(
      isPasteShortcut({ ...key, ctrlKey: false, metaKey: true }, true),
    ).toBe(true);
    for (const modifiers of [
      { shiftKey: true },
      { altKey: true },
      { metaKey: true },
    ]) {
      expect(isPasteShortcut({ ...key, ...modifiers }, false)).toBe(false);
    }
  });
  it("leaves text inputs and contenteditable elements alone", () => {
    for (const target of [
      { tagName: "INPUT" },
      { tagName: "TEXTAREA" },
      { isContentEditable: true },
    ]) {
      expect(isEditablePasteTarget(target as unknown as EventTarget)).toBe(
        true,
      );
    }
    expect(isEditablePasteTarget(null)).toBe(false);
  });
});

describe("native clipboard copy boundary", () => {
  function options() {
    return {
      root: "/repo",
      workspace: { kind: "local" as const },
      isCurrent: () => true,
      refresh: vi.fn(async () => true),
    };
  }
  it("never asks the server for a browser's OS clipboard", async () => {
    h.native = false;
    expect(await pasteClipboardFiles("/repo", options())).toEqual([]);
    expect(h.invoke).not.toHaveBeenCalled();
  });
  it("does no work for an expired scope", async () => {
    await pasteClipboardFiles("/repo", {
      ...options(),
      isCurrent: () => false,
    });
    expect(h.invoke).not.toHaveBeenCalled();
  });
  it("does not copy after the workspace changes during the clipboard read", async () => {
    let current = true;
    h.invoke.mockImplementationOnce(async () => {
      current = false;
      return ["/source/a"];
    });
    const opts = { ...options(), isCurrent: () => current };
    expect(await pasteClipboardFiles("/repo", opts)).toEqual([]);
    expect(h.invoke).toHaveBeenCalledExactlyOnceWith("fs_clipboard_paths");
    expect(opts.refresh).not.toHaveBeenCalled();
  });
  it("reports an empty file clipboard without invoking copy", async () => {
    h.invoke.mockResolvedValueOnce([]);
    await pasteClipboardFiles("/repo", options());
    expect(h.message).toHaveBeenCalledWith("No files on the clipboard");
    expect(h.invoke).toHaveBeenCalledOnce();
  });
  it("uses captured authorization and renaming, and refreshes partial success", async () => {
    h.invoke.mockResolvedValueOnce(["/source/a"]).mockResolvedValueOnce({
      created: ["/repo/a copy"],
      failed: [{ source: "/b", message: "missing" }],
    });
    const opts = options();
    expect(await pasteClipboardFiles("/repo", opts)).toEqual(["/repo/a copy"]);
    expect(h.invoke).toHaveBeenLastCalledWith("fs_copy", {
      sources: ["/source/a"],
      destDir: "/repo",
      root: "/repo",
      workspace: opts.workspace,
      renameOnConflict: true,
    });
    expect(opts.refresh).toHaveBeenCalledExactlyOnceWith("/repo");
    expect(h.error).toHaveBeenCalledWith("Some items could not be copied", {
      description: "/b: missing",
    });
  });
  it("does not refresh a new workspace after an old copy completes", async () => {
    let current = true;
    h.invoke
      .mockResolvedValueOnce(["/source/a"])
      .mockImplementationOnce(async () => {
        current = false;
        return { created: ["/repo/a"], failed: [] };
      });
    const opts = { ...options(), isCurrent: () => current };
    expect(await pasteClipboardFiles("/repo", opts)).toEqual([]);
    expect(opts.refresh).not.toHaveBeenCalled();
  });
  it("does not refresh a batch where every file failed", async () => {
    h.invoke.mockResolvedValueOnce(["/source/a"]).mockResolvedValueOnce({
      created: [],
      failed: [{ source: "/source/a", message: "no access" }],
    });
    const opts = options();
    expect(await pasteClipboardFiles("/repo", opts)).toEqual([]);
    expect(opts.refresh).not.toHaveBeenCalled();
  });
  it("returns no stale selection when the scope changes during refresh", async () => {
    let current = true;
    h.invoke
      .mockResolvedValueOnce(["/source/a"])
      .mockResolvedValueOnce({ created: ["/repo/a"], failed: [] });
    expect(
      await pasteClipboardFiles("/repo", {
        ...options(),
        isCurrent: () => current,
        refresh: async () => {
          current = false;
          return true;
        },
      }),
    ).toEqual([]);
  });
  it("does not retry a failed refresh or leak a rejection", async () => {
    h.invoke
      .mockResolvedValueOnce(["/source/a"])
      .mockResolvedValueOnce({ created: ["/repo/a"], failed: [] });
    const opts = {
      ...options(),
      refresh: vi.fn(async () => {
        throw new Error("refresh failed");
      }),
    };
    expect(await pasteClipboardFiles("/repo", opts)).toEqual([]);
    expect(opts.refresh).toHaveBeenCalledOnce();
    expect(h.error).toHaveBeenCalledWith(
      "Files copied, but directory refresh failed: Error: refresh failed",
    );
  });
  it("tries one refresh after an uncertain copy failure and contains its failure too", async () => {
    h.invoke
      .mockResolvedValueOnce(["/source/a"])
      .mockRejectedValueOnce(new Error("disconnected"));
    const opts = {
      ...options(),
      refresh: vi.fn(async () => {
        throw new Error("disconnected");
      }),
    };
    expect(await pasteClipboardFiles("/repo", opts)).toEqual([]);
    expect(opts.refresh).toHaveBeenCalledOnce();
  });
});
