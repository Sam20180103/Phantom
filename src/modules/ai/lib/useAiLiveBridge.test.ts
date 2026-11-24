import { beforeEach, describe, expect, it, vi } from "vitest";
import type { TerminalTab } from "@/modules/tabs/lib/useTabs";
import type { TerminalPaneHandle } from "@/modules/terminal/TerminalPane";
import { useAiLiveBridge } from "@/modules/ai/lib/useAiLiveBridge";

const h = vi.hoisted(() => ({
  ref: null as { current: unknown } | null,
  effect: null as (() => void) | null,
}));
vi.mock("react", () => ({
  useRef: (value: unknown) => {
    h.ref ??= { current: value };
    return h.ref;
  },
  useEffect: (effect: () => void) => {
    h.effect ??= effect;
  },
}));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@/modules/terminal", async () => ({
  findLeafCwd: (await import("@/modules/terminal/lib/panes")).findLeafCwd,
  whenSessionReady: vi.fn(),
  writeToSession: vi.fn(),
}));
vi.mock("@/modules/agents/store/managedAgentsStore", () => ({
  useManagedAgentsStore: { getState: vi.fn() },
}));

beforeEach(() => {
  h.ref = null;
  h.effect = null;
});

type Params = Parameters<typeof useAiLiveBridge>[0];
function useBridgeFixture() {
  const read = vi.fn(() => "API_KEY=secret-value\npublic output");
  const terminal: TerminalTab = {
    id: 1,
    kind: "terminal",
    spaceId: "test",
    title: "shell",
    activeLeafId: 10,
    paneTree: { kind: "leaf", id: 10 },
  };
  const params: Params = {
    setLive: vi.fn(),
    activeId: 1,
    tabs: [terminal],
    explorerRoot: "/repo",
    launchCwd: null,
    home: null,
    openPreviewTab: vi.fn(),
    newAgentTab: vi.fn(() => ({ tabId: 2, leafId: 20 })),
    terminalRefs: {
      current: new Map([
        [10, { getBuffer: read } as unknown as TerminalPaneHandle],
      ]),
    },
  };
  useAiLiveBridge(params);
  h.effect?.();
  const live = vi.mocked(params.setLive).mock.calls[0][0];
  return { params, terminal, read, live };
}

describe("live AI terminal bridge", () => {
  it("redacts both the active-terminal and arbitrary-leaf read paths", () => {
    const { live, read } = useBridgeFixture();
    for (const context of [
      live.getTerminalContext(),
      live.readLeafBuffer(10),
    ]) {
      expect(context).toContain("API_KEY=<REDACTED>");
      expect(context).not.toContain("secret-value");
    }
    expect(read.mock.calls).toEqual([[300], [300]]);
  });

  it("applies privacy changes immediately to both getters without republishing", () => {
    const { params, terminal, read, live } = useBridgeFixture();
    useAiLiveBridge({ ...params, tabs: [{ ...terminal, private: true }] });
    expect(live.getTerminalContext()).toBeNull();
    expect(live.readLeafBuffer(10)).toBeNull();
    expect(live.isActiveTerminalPrivate()).toBe(true);
    expect(read).not.toHaveBeenCalled();
    expect(params.setLive).toHaveBeenCalledOnce();
  });

  it("does not read a stale terminal ref after its tab closes", () => {
    const { params, read, live } = useBridgeFixture();
    useAiLiveBridge({ ...params, tabs: [] });
    expect(live.readLeafBuffer(10)).toBeNull();
    expect(live.getTerminalContext()).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("follows the newly active split pane rather than a previously captured one", () => {
    const { params, terminal, read, live } = useBridgeFixture();
    const other = vi.fn(() => "new split output");
    params.terminalRefs.current.set(11, {
      getBuffer: other,
    } as unknown as TerminalPaneHandle);
    useAiLiveBridge({
      ...params,
      tabs: [
        {
          ...terminal,
          activeLeafId: 11,
          paneTree: {
            kind: "split",
            id: 3,
            dir: "row",
            children: [
              { kind: "leaf", id: 10 },
              { kind: "leaf", id: 11 },
            ],
          },
        },
      ],
    });
    expect(live.getTerminalContext()).toBe("new split output");
    expect(other).toHaveBeenCalledExactlyOnceWith(300);
    expect(read).not.toHaveBeenCalled();
  });
});
