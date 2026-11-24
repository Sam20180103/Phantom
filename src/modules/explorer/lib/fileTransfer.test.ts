import { afterEach, describe, expect, it, vi } from "vitest";
import { uploadFiles } from "@/modules/explorer/lib/fileTransfer";
import type { WorkspaceEnv } from "@/modules/workspace";

const h = vi.hoisted(() => ({ workspace: { kind: "local" } as WorkspaceEnv }));
vi.mock("@/modules/workspace", () => ({
  currentWorkspaceEnv: () => h.workspace,
}));
afterEach(() => vi.unstubAllGlobals());

describe("browser upload workspace snapshot", () => {
  it("keeps every file in the original WSL distro after a switch", async () => {
    h.workspace = { kind: "wsl", distro: "first" };
    const fetch = vi.fn(async () => {
      h.workspace = { kind: "wsl", distro: "second" };
      return Response.json({ path: "/repo/file", name: "file" });
    });
    vi.stubGlobal("fetch", fetch);
    await uploadFiles("/repo", [new File(["a"], "a"), new File(["b"], "b")]);
    expect(fetch).toHaveBeenCalledTimes(2);
    for (const call of vi.mocked(globalThis.fetch).mock.calls) {
      expect(
        new URL(String(call[0]), "http://localhost").searchParams.get("distro"),
      ).toBe("first");
    }
  });

  it("does not send a local upload's remaining files into a newly selected distro", async () => {
    h.workspace = { kind: "local" };
    const urls: string[] = [];
    vi.stubGlobal("fetch", async (url: string) => {
      urls.push(url);
      h.workspace = { kind: "wsl", distro: "other" };
      return Response.json({ path: "/repo/file", name: "file" });
    });
    await uploadFiles("/repo", [new File(["a"], "a"), new File(["b"], "b")]);
    expect(urls).toHaveLength(2);
    expect(
      urls.every(
        (url) => !new URL(url, "http://localhost").searchParams.has("distro"),
      ),
    ).toBe(true);
  });
});
