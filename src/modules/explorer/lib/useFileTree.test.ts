import { describe, expect, it } from "vitest";
import { batchRootPaths, planBatchMove } from "@/modules/explorer/lib/batchPaths";

describe("planBatchMove", () => {
  it("moves an ancestor only once when descendants are also selected", () => {
    expect(planBatchMove(["/repo/src/a.ts", "/repo/src"], "/repo/dest", [])).toEqual([
      { from: "/repo/src", to: "/repo/dest/src", name: "src", conflict: false },
    ]);
  });

  it("rejects recursive destinations even outside the drag handler", () => {
    expect(planBatchMove(["/repo/src"], "/repo/src/nested", [])).toEqual([]);
    expect(planBatchMove(["/repo/src"], "/repo/src", [])).toEqual([]);
  });

  it("normalizes Windows paths and trailing destination separators", () => {
    expect(planBatchMove(["C:\\repo\\src\\a.ts"], "C:\\repo\\dest\\", [])).toEqual([
      { from: "C:/repo/src/a.ts", to: "C:/repo/dest/a.ts", name: "a.ts", conflict: false },
    ]);
  });

  it("plans a conflict-free batch with no collisions", () => {
    const items = planBatchMove(
      ["/repo/src/a.ts", "/repo/other/b.ts"],
      "/repo/dest",
      ["existing.ts"],
    );
    expect(items).toEqual([
      { from: "/repo/src/a.ts", to: "/repo/dest/a.ts", name: "a.ts", conflict: false },
      { from: "/repo/other/b.ts", to: "/repo/dest/b.ts", name: "b.ts", conflict: false },
    ]);
  });

  it("flags a destination collision against an existing entry", () => {
    const items = planBatchMove(["/repo/src/a.ts"], "/repo/dest", ["a.ts"]);
    expect(items).toEqual([
      { from: "/repo/src/a.ts", to: "/repo/dest/a.ts", name: "a.ts", conflict: true },
    ]);
  });

  it("flags a within-batch basename collision, earlier item wins the name", () => {
    const items = planBatchMove(
      ["/repo/src/a.ts", "/repo/other/a.ts"],
      "/repo/dest",
      [],
    );
    expect(items).toEqual([
      { from: "/repo/src/a.ts", to: "/repo/dest/a.ts", name: "a.ts", conflict: false },
      { from: "/repo/other/a.ts", to: "/repo/dest/a.ts", name: "a.ts", conflict: true },
    ]);
  });

  it("drops sources already directly in the target as no-ops", () => {
    const items = planBatchMove(
      ["/repo/dest/already-there.ts", "/repo/src/new.ts"],
      "/repo/dest",
      ["already-there.ts"],
    );
    expect(items).toEqual([
      {
        from: "/repo/src/new.ts",
        to: "/repo/dest/new.ts",
        name: "new.ts",
        conflict: false,
      },
    ]);
  });

  it("handles mixed-depth sources landing in the same target", () => {
    const items = planBatchMove(
      ["/repo/a.ts", "/repo/deep/nested/dir/b.ts"],
      "/repo/dest",
      [],
    );
    expect(items.map((i) => i.to)).toEqual([
      "/repo/dest/a.ts",
      "/repo/dest/b.ts",
    ]);
  });
});

describe("batchRootPaths", () => {
  it("deduplicates paths and removes descendants regardless of selection order", () => {
    expect(batchRootPaths(["/repo/src/a.ts", "/repo/src/", "/repo/src", "/repo/other.ts"])).toEqual([
      "/repo/src", "/repo/other.ts",
    ]);
  });

  it("does not mistake a shared name prefix for an ancestor", () => {
    expect(batchRootPaths(["/repo/src", "/repo/src-other/a.ts"])).toEqual([
      "/repo/src", "/repo/src-other/a.ts",
    ]);
  });

  it("keeps empty batches empty", () => {
    expect(batchRootPaths([])).toEqual([]);
  });

  it("keeps a root selection and absorbs its descendants", () => {
    expect(batchRootPaths(["/repo/a.ts", "/"])).toEqual(["/"]);
  });
});
