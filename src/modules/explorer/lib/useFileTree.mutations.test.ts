import { beforeEach, describe, expect, it, vi } from "vitest";
import { useFileTree } from "@/modules/explorer/lib/useFileTree";

const harness = vi.hoisted(() => ({
  invoke: vi.fn(),
  error: vi.fn(),
  warning: vi.fn(),
  workspace: { kind: "local" },
}));

vi.mock("react", () => ({
  useRef: (current: unknown) => ({ current }),
  useState: (initial: unknown) => [initial, vi.fn()],
  useEffect: vi.fn(),
  useLayoutEffect: vi.fn(),
  useMemo: (factory: () => unknown) => factory(),
  useCallback: (callback: unknown) => callback,
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: harness.invoke }));
vi.mock("sonner", () => ({
  toast: { error: harness.error, warning: harness.warning },
}));
vi.mock("@/modules/workspace", () => ({
  currentWorkspaceEnv: () => harness.workspace,
  useWorkspaceEnvStore: (selector: (state: unknown) => unknown) => selector({ env: harness.workspace }),
  workspaceScopeKey: (workspace: { kind: string }) => workspace.kind,
}));
vi.mock("@/modules/settings/preferences", () => ({
  usePreferencesStore: (selector: (state: unknown) => unknown) =>
    selector({
      showHidden: false,
      explorerGitDecorations: false,
    }),
}));
vi.mock("@/modules/explorer/lib/watch", () => ({
  listenFsChanged: vi.fn(),
  watchAdd: vi.fn(),
  watchRemove: vi.fn(),
}));

beforeEach(() => {
  vi.resetAllMocks();
  harness.workspace = { kind: "local" };
  harness.invoke.mockImplementation(async (command) =>
    command === "fs_read_dir" ? [] : undefined,
  );
});

describe("file tree batch mutations", () => {
  it("deletes an ancestor once without racing selected descendants", async () => {
    const onPathDeleted = vi.fn();
    const tree = useFileTree("/repo", { onPathDeleted });
    await tree.deletePaths(["/repo/src/a.ts", "/repo/src", "/repo/src"]);
    expect(
      harness.invoke.mock.calls.filter(([command]) => command === "fs_delete"),
    ).toEqual([
      ["fs_delete", { path: "/repo/src", workspace: { kind: "local" } }],
    ]);
    expect(onPathDeleted).toHaveBeenCalledExactlyOnceWith("/repo/src");
  });

  it("reads all destination names before moving, including hidden files", async () => {
    harness.invoke.mockResolvedValueOnce([{ name: ".env" }]);
    harness.warning.mockImplementation((_message, options) =>
      options.cancel.onClick(),
    );
    const tree = useFileTree("/repo");
    await tree.movePaths(["/repo/src/.env"], "/repo/dest");
    expect(harness.invoke).toHaveBeenCalledWith("fs_read_dir", {
      path: "/repo/dest",
      showHidden: true,
      gitDecorations: false,
      workspace: { kind: "local" },
    });
    expect(harness.warning).toHaveBeenCalledOnce();
    expect(
      harness.invoke.mock.calls.some(
        ([command]) => command === "fs_delete" || command === "fs_rename",
      ),
    ).toBe(false);
  });

  it("does not mutate files when the destination cannot be read", async () => {
    harness.invoke.mockRejectedValueOnce(new Error("permission denied"));
    const tree = useFileTree("/repo");
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      await tree.movePaths(["/repo/src/a.ts"], "/repo/dest");
      expect(harness.invoke).toHaveBeenCalledOnce();
      expect(harness.error).toHaveBeenCalledWith(
        "Could not read the destination directory",
      );
    } finally {
      log.mockRestore();
    }
  });

  it("keeps the original workspace throughout a confirmed replacement", async () => {
    harness.invoke.mockResolvedValueOnce([{ name: "a.ts" }]);
    harness.warning.mockImplementation((_message, options) => {
      harness.workspace = { kind: "changed" };
      options.action.onClick();
    });
    const onPathRenamed = vi.fn();
    const tree = useFileTree("/repo", { onPathRenamed });
    await tree.movePaths(["/repo/src/a.ts"], "/repo/dest");
    const mutations = harness.invoke.mock.calls.filter(
      ([command]) => command === "fs_rename" || command === "fs_delete",
    );
    expect(mutations).toHaveLength(3);
    expect(mutations.every(([, args]) => args.workspace.kind === "local")).toBe(
      true,
    );
    expect(mutations[0][1]).toMatchObject({
      from: "/repo/dest/a.ts",
      to: expect.stringContaining("/repo/dest/.phantom-move-"),
    });
    expect(mutations[1][1]).toMatchObject({
      from: "/repo/src/a.ts",
      to: "/repo/dest/a.ts",
    });
    expect(mutations[2][1].path).toBe(mutations[0][1].to);
    expect(onPathRenamed).toHaveBeenCalledExactlyOnceWith(
      "/repo/src/a.ts",
      "/repo/dest/a.ts",
    );
  });

  it("reports partial move failure while preserving successful callbacks", async () => {
    harness.invoke.mockImplementation(async (command, args) => {
      if (command === "fs_read_dir") return [];
      if (args.from === "/repo/src/b.ts") throw new Error("permission denied");
    });
    const onPathRenamed = vi.fn();
    const tree = useFileTree("/repo", { onPathRenamed });
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    try {
      await tree.movePaths(["/repo/src/a.ts", "/repo/src/b.ts"], "/repo/dest");
      expect(onPathRenamed).toHaveBeenCalledExactlyOnceWith(
        "/repo/src/a.ts",
        "/repo/dest/a.ts",
      );
      expect(harness.error).toHaveBeenCalledWith(
        "Move failed for one or more items",
      );
    } finally {
      log.mockRestore();
    }
  });
});
