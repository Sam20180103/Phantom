import { describe, expect, it } from "vitest";
import { createTreeReadScope } from "@/modules/explorer/lib/treeReadScope";

describe("tree read lifetime", () => {
  it("invalidates removed subtrees without cancelling siblings", () => {
    const scope = createTreeReadScope();
    scope.activate();
    const removed = scope.begin("/repo/src/nested");
    const sibling = scope.begin("/repo/src-other");
    scope.cancelUnder(["/repo/src"]);
    expect(removed?.isCurrent()).toBe(false);
    expect(sibling?.isCurrent()).toBe(true);
  });

  it("starts no reads before activation or after suspension", () => {
    const scope = createTreeReadScope();
    expect(scope.begin("/repo")).toBeNull();
    scope.activate();
    expect(scope.begin("/repo")).not.toBeNull();
    scope.suspend();
    expect(scope.begin("/repo")).toBeNull();
  });

  it("does not revive old responses when the explorer is shown again", () => {
    const scope = createTreeReadScope();
    scope.activate();
    const old = scope.begin("/repo");
    scope.suspend();
    scope.activate();
    const fresh = scope.begin("/repo");
    expect(old?.isCurrent()).toBe(false);
    expect(fresh?.isCurrent()).toBe(true);
    old?.finish();
    expect(fresh?.isCurrent()).toBe(true);
  });

  it("allows only the latest response for each directory", () => {
    const scope = createTreeReadScope();
    scope.activate();
    const old = scope.begin("/repo");
    const child = scope.begin("/repo/src");
    const fresh = scope.begin("/repo");
    expect(old?.isCurrent()).toBe(false);
    expect(child?.isCurrent()).toBe(true);
    expect(fresh?.isCurrent()).toBe(true);
    fresh?.finish();
    expect(fresh?.isCurrent()).toBe(false);
    expect(old?.isCurrent()).toBe(false);
  });
});
