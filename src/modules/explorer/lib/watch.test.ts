import { expect, it, vi } from "vitest";

const invoke = vi.hoisted(() =>
  vi.fn(async (_command: string, _args: unknown) => {}),
);
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: vi.fn(),
}));
vi.mock("@/modules/workspace", () => ({
  currentWorkspaceEnv: () => ({ kind: "local" }),
}));

it("keeps release behind pending registration and continues after failure", async () => {
  const { watchAdd, watchRemove } = await import("@/modules/explorer/lib/watch");
  let rejectAdd!: (error: Error) => void;
  invoke.mockImplementationOnce(
    () =>
      new Promise<void>((_resolve, reject) => {
        rejectAdd = reject;
      }),
  );
  watchAdd(["/repo"], ["/repo/a.txt"]);
  watchRemove(["/repo"], ["/repo/a.txt"]);
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledOnce());
  expect(invoke.mock.calls[0]).toEqual([
    "fs_watch_add",
    {
      paths: ["/repo"],
      files: ["/repo/a.txt"],
      workspace: { kind: "local" },
    },
  ]);
  rejectAdd(new Error("registration failed"));
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(2));
  expect(invoke.mock.calls[1][0]).toBe("fs_watch_remove");
  watchAdd([], ["/repo/b.txt"]);
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledTimes(3));
  expect(invoke.mock.calls[2][1]).toMatchObject({
    paths: [],
    files: ["/repo/b.txt"],
  });
});
