import { describe, expect, it } from "vitest";
import type { PaneNode } from "@/modules/terminal/lib/panes";
import type { Tab } from "@/modules/tabs/lib/useTabs";
import { hydrateTabs, serializeTabs, type SerializedTab } from "./serialize";

function counter(start = 100): () => number {
  let n = start;
  return () => n++;
}

function leafIdsOf(node: PaneNode): number[] {
  return node.kind === "leaf" ? [node.id] : node.children.flatMap(leafIdsOf);
}

function term(over: Partial<Extract<Tab, { kind: "terminal" }>>): Tab {
  return {
    id: 1,
    kind: "terminal",
    spaceId: "s1",
    title: "shell",
    paneTree: { kind: "leaf", id: 2, cwd: "/a" },
    activeLeafId: 2,
    ...over,
  } as Tab;
}

describe("serializeTabs", () => {
  it("drops private terminals and transient kinds", () => {
    const tabs: Tab[] = [
      term({ id: 1 }),
      term({ id: 3, private: true }),
      {
        id: 5,
        kind: "git-diff",
        spaceId: "s1",
        title: "d",
        path: "/a/x",
        repoRoot: "/a",
        mode: "+",
        originalPath: null,
        preview: true,
      },
      {
        id: 7,
        kind: "editor",
        spaceId: "s1",
        title: "x",
        path: "/a/x.ts",
        dirty: false,
        preview: false,
      },
    ];
    const out = serializeTabs(tabs);
    expect(out.map((t) => t.kind)).toEqual(["terminal", "editor"]);
  });

  it("persists a leaf's session name", () => {
    const [s] = serializeTabs([
      term({ paneTree: { kind: "leaf", id: 2, cwd: "/a", session: "phantom-x" } }),
    ]);
    const node = s as Extract<SerializedTab, { kind: "terminal" }>;
    expect(node.tree).toMatchObject({ session: "phantom-x" });
  });

  it("never persists an agent's reserved name as a session", () => {
    // `pty_open` refuses `phantom-agent-*`. Persisting one meant the refusal
    // came back on every restore and the retry stranded a fresh shell — the
    // session leak the reattach banner used to cause.
    const [tab] = serializeTabs([
      term({ paneTree: { kind: "leaf", id: 2, attach: "phantom-agent-agy" } }),
    ]);
    const node = tab as Extract<SerializedTab, { kind: "terminal" }>;
    expect(node.tree).not.toHaveProperty("session");

    // And restoring it yields a normal, reusable terminal rather than nothing.
    const [restored] = hydrateTabs([node], "s1", counter());
    const tree = (restored as Extract<Tab, { kind: "terminal" }>).paneTree;
    expect(tree.kind === "leaf" && tree.session).toMatch(/^phantom-/);
    expect(tree.kind === "leaf" && tree.session).not.toMatch(/^phantom-agent-/);
  });

  it("persists a reattach tab's adopted name as its session", () => {
    // A banner reattach carries the name in `attach`, never `session`; without
    // the fallback it would leak a session on every reload.
    const [s] = serializeTabs([
      term({ paneTree: { kind: "leaf", id: 2, attach: "phantom-1-2" } }),
    ]);
    const node = s as Extract<SerializedTab, { kind: "terminal" }>;
    expect(node.tree).toMatchObject({ session: "phantom-1-2" });
  });

  it("marks the active leaf in a split tree", () => {
    const tree: PaneNode = {
      kind: "split",
      id: 10,
      dir: "row",
      children: [
        { kind: "leaf", id: 11, cwd: "/a" },
        { kind: "leaf", id: 12, cwd: "/b" },
      ],
    };
    const [s] = serializeTabs([term({ paneTree: tree, activeLeafId: 12 })]);
    const node = s as Extract<SerializedTab, { kind: "terminal" }>;
    expect(node.tree.kind).toBe("split");
    if (node.tree.kind === "split") {
      expect(node.tree.children[1]).toMatchObject({ cwd: "/b", active: true });
      expect(node.tree.children[0]).not.toHaveProperty("active");
    }
  });
});

describe("hydrateTabs", () => {
  it("round-trips structure, cwd, blocks and active leaf", () => {
    const tree: PaneNode = {
      kind: "split",
      id: 10,
      dir: "col",
      children: [
        { kind: "leaf", id: 11, cwd: "/a" },
        { kind: "leaf", id: 12, cwd: "/b" },
      ],
    };
    const tabs: Tab[] = [
      term({
        paneTree: tree,
        activeLeafId: 12,
        blocks: true,
        customTitle: "x",
      }),
    ];
    const serialized = serializeTabs(tabs);
    const [restored] = hydrateTabs(serialized, "s2", counter());
    expect(restored.kind).toBe("terminal");
    if (restored.kind !== "terminal") return;

    expect(restored.spaceId).toBe("s2");
    expect(restored.cold).toBe(true);
    expect(restored.blocks).toBe(true);
    expect(restored.customTitle).toBe("x");
    expect(restored.paneTree.kind).toBe("split");

    const leaves = leafIdsOf(restored.paneTree);
    expect(new Set(leaves).size).toBe(2);
    expect(leaves).toContain(restored.activeLeafId);
    // active leaf was the second one, which carried /b
    expect(restored.cwd).toBe("/b");
  });

  it("allocates fresh, unique, monotonic ids across all tabs and leaves", () => {
    const tree: PaneNode = {
      kind: "split",
      id: 10,
      dir: "row",
      children: [
        { kind: "leaf", id: 11, cwd: "/a" },
        { kind: "leaf", id: 12, cwd: "/b" },
      ],
    };
    const serialized = serializeTabs([
      term({ id: 1, paneTree: tree, activeLeafId: 11 }),
      term({ id: 2 }),
    ]);
    const restored = hydrateTabs(serialized, "s1", counter(100));

    const ids: number[] = [];
    for (const t of restored) {
      ids.push(t.id);
      if (t.kind === "terminal") ids.push(...leafIdsOf(t.paneTree));
    }
    expect(new Set(ids).size).toBe(ids.length);
    expect(Math.min(...ids)).toBeGreaterThanOrEqual(100);
  });

  it("reuses a persisted session name and mints one when absent", () => {
    // Persisted name survives the round-trip, so reload reattaches the shell.
    const withName = hydrateTabs(
      [{ kind: "terminal", tree: { kind: "leaf", session: "phantom-keep" } }],
      "s1",
      counter(),
    )[0];
    if (withName.kind !== "terminal" || withName.paneTree.kind !== "leaf") {
      throw new Error("expected a terminal leaf");
    }
    expect(withName.paneTree.session).toBe("phantom-keep");

    // Older snapshot without a name gets a fresh phantom- one (spawns once).
    const noName = hydrateTabs(
      [{ kind: "terminal", tree: { kind: "leaf" } }],
      "s1",
      counter(),
    )[0];
    if (noName.kind !== "terminal" || noName.paneTree.kind !== "leaf") {
      throw new Error("expected a terminal leaf");
    }
    expect(noName.paneTree.session).toMatch(/^phantom-/);
  });

  it("returns empty for corrupted input without throwing", () => {
    expect(hydrateTabs([] as SerializedTab[], "s1", counter())).toEqual([]);
    expect(
      hydrateTabs(null as unknown as SerializedTab[], "s1", counter()),
    ).toEqual([]);
  });

  it("hydrates editor/preview/markdown as cold with derived titles", () => {
    const serialized: SerializedTab[] = [
      { kind: "editor", path: "/a/foo.ts" },
      { kind: "preview", url: "http://localhost:5173/x" },
      { kind: "markdown", path: "/a/README.md" },
    ];
    const out = hydrateTabs(serialized, "s1", counter());
    expect(out.every((t) => t.cold === true)).toBe(true);
    expect(out.map((t) => t.title)).toEqual([
      "foo.ts",
      "localhost:5173",
      "README.md",
    ]);
  });
});
