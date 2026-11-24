import { describe, expect, it, vi } from "vitest";
import type { TerminalTab } from "@/modules/tabs/lib/useTabs";
import { useWorkspaceCwd } from "@/modules/tabs/lib/useWorkspaceCwd";

vi.mock("react", () => ({
  useRef: (current: unknown) => ({ current }),
  useEffect: () => {},
  useMemo: (compute: () => unknown) => compute(),
  useCallback: (callback: unknown) => callback,
}));

const other: TerminalTab = {
  id: 1, kind: "terminal", spaceId: "s", title: "other", cwd: "/other/project",
  paneTree: { kind: "leaf", id: 2 }, activeLeafId: 2,
};
const attached: TerminalTab = {
  id: 3, kind: "terminal", spaceId: "s", title: "session",
  paneTree: { kind: "leaf", id: 4, attach: "phantom-existing" }, activeLeafId: 4,
};

describe("active terminal workspace", () => {
  it("does not show another terminal's files while an attached cwd is unknown", () => {
    const result = useWorkspaceCwd(attached, [other, attached], "/home/user");
    expect(result.explorerRoot).toBeNull();
  });

  it("uses the cwd recovered for the active terminal", () => {
    const ready = { ...attached, cwd: "/path/to/Phantom" };
    const result = useWorkspaceCwd(ready, [other, ready], "/home/user");
    expect(result.explorerRoot).toBe(ready.cwd);
    expect(result.inheritedCwdForNewTab()).toBe(ready.cwd);
  });
});
