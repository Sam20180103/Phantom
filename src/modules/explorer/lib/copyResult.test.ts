import { describe, expect, it } from "vitest";
import { copyFailureNotice } from "@/modules/explorer/lib/copyResult";

describe("copy failure notices", () => {
  it("is silent when all items succeed or no files were supplied", () => {
    expect(copyFailureNotice({ created: ["/a"], failed: [] })).toBeNull();
    expect(copyFailureNotice({ created: [], failed: [] })).toBeNull();
  });

  it("distinguishes partial success from total failure", () => {
    const failed = [{ source: "/a", message: "missing" }];
    expect(copyFailureNotice({ created: ["/b"], failed })).toEqual({
      message: "Some items could not be copied",
      description: "/a: missing",
    });
    expect(copyFailureNotice({ created: [], failed })?.message).toBe("Copy failed");
  });

  it("bounds the displayed list without hiding the total failure count", () => {
    const failed = Array.from({ length: 8 }, (_, n) => ({
      source: `/${n}`,
      message: "failed",
    }));
    const notice = copyFailureNotice({ created: [], failed });
    expect(notice?.description.split("\n")).toHaveLength(4);
    expect(notice?.description).toContain("And 5 more");
  });
});
