import { describe, expect, it, vi } from "vitest";
import type { TerminalTab } from "@/modules/tabs/lib/useTabs";
import { readTerminalContext } from "@/modules/ai/lib/terminalContext";

function tab(overrides: Partial<TerminalTab> = {}): TerminalTab {
  return {
    id: 1,
    kind: "terminal",
    spaceId: "test",
    title: "shell",
    activeLeafId: 10,
    paneTree: { kind: "leaf", id: 10 },
    ...overrides,
  };
}

describe("AI terminal context ownership", () => {
  it("reads at most 300 lines from an owned leaf and redacts detected secrets", () => {
    const read = vi.fn(
      () =>
        "中文 output\nAPI_KEY=super-secret\nBearer abcdefghijklmnopqrstuvwxyz",
    );
    expect(readTerminalContext([tab()], 10, read)).toBe(
      "中文 output\nAPI_KEY=<REDACTED>\n<REDACTED:bearer>",
    );
    expect(read).toHaveBeenCalledExactlyOnceWith(10, 300);
  });

  it("never reads a private tab through the arbitrary-leaf API", () => {
    const read = vi.fn(() => "private output");
    expect(readTerminalContext([tab({ private: true })], 10, read)).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("protects every pane of a private split, not only the active pane", () => {
    const split = tab({
      private: true,
      paneTree: {
        kind: "split",
        id: 3,
        dir: "row",
        children: [
          { kind: "leaf", id: 10 },
          { kind: "leaf", id: 11 },
        ],
      },
    });
    const read = vi.fn(() => "private split");
    expect(readTerminalContext([split], 11, read)).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("allows an inactive public split pane without substituting the active leaf", () => {
    const split = tab({
      paneTree: {
        kind: "split",
        id: 3,
        dir: "row",
        children: [
          { kind: "leaf", id: 10 },
          { kind: "leaf", id: 11 },
        ],
      },
    });
    const read = vi.fn((id: number) => `pane ${id}`);
    expect(readTerminalContext([split], 11, read)).toBe("pane 11");
    expect(read).toHaveBeenCalledExactlyOnceWith(11, 300);
  });

  it("ignores closed or unowned leaf refs", () => {
    const read = vi.fn(() => "stale ref");
    expect(readTerminalContext([], 10, read)).toBeNull();
    expect(readTerminalContext([tab()], 999, read)).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("fails closed when duplicate leaf ids have ambiguous privacy ownership", () => {
    const read = vi.fn(() => "ambiguous");
    expect(
      readTerminalContext([tab(), tab({ id: 2, private: true })], 10, read),
    ).toBeNull();
    expect(read).not.toHaveBeenCalled();
  });

  it("rechecks privacy on every call instead of retaining an old permission", () => {
    const read = vi.fn(() => "visible");
    expect(readTerminalContext([tab()], 10, read)).toBe("visible");
    expect(readTerminalContext([tab({ private: true })], 10, read)).toBeNull();
    expect(read).toHaveBeenCalledOnce();
  });

  it.each([null, undefined, ""])("handles missing buffer %s", (buffer) => {
    expect(readTerminalContext([tab()], 10, () => buffer)).toBeNull();
  });
});
