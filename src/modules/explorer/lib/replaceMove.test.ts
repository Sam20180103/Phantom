import { describe, expect, it, vi } from "vitest";
import { replaceMove } from "@/modules/explorer/lib/replaceMove";

function fakeFs() {
  const files = new Map([
    ["/source", "new"],
    ["/target", "original"],
  ]);
  return {
    files,
    rename: vi.fn(async (from: string, to: string) => {
      const content = files.get(from);
      if (content === undefined) throw new Error("missing source");
      if (files.has(to)) throw new Error("destination occupied");
      files.set(to, content);
      files.delete(from);
    }),
    remove: vi.fn(async (path: string) => {
      files.delete(path);
    }),
  };
}

describe("replaceMove", () => {
  it("preserves the original until the move succeeds, then removes only the backup", async () => {
    const fs = fakeFs();
    expect(await replaceMove("/source", "/target", "/backup", fs)).toBeNull();
    expect(fs.files).toEqual(new Map([["/target", "new"]]));
    expect(fs.remove).toHaveBeenCalledExactlyOnceWith("/backup");
  });

  it("restores the destination when the source move fails", async () => {
    const fs = fakeFs();
    fs.rename
      .mockImplementationOnce(async () => {
        fs.files.set("/backup", "original");
        fs.files.delete("/target");
      })
      .mockRejectedValueOnce(new Error("cross-device move"));
    await expect(
      replaceMove("/source", "/target", "/backup", fs),
    ).rejects.toThrow("cross-device move");
    expect(fs.files).toEqual(
      new Map([
        ["/source", "new"],
        ["/target", "original"],
      ]),
    );
    expect(fs.remove).not.toHaveBeenCalled();
  });

  it("never removes the backup if a concurrent target prevents restoration", async () => {
    const fs = fakeFs();
    fs.rename
      .mockImplementationOnce(async () => {
        fs.files.set("/backup", "original");
        fs.files.set("/target", "concurrent");
      })
      .mockRejectedValueOnce(new Error("move failed"));
    await expect(
      replaceMove("/source", "/target", "/backup", fs),
    ).rejects.toThrow("Original destination preserved at /backup");
    expect(fs.files.get("/backup")).toBe("original");
    expect(fs.files.get("/target")).toBe("concurrent");
    expect(fs.remove).not.toHaveBeenCalled();
  });

  it("reports retained backups without treating a completed move as failed", async () => {
    const fs = fakeFs();
    fs.remove.mockRejectedValueOnce(new Error("permission denied"));
    expect(await replaceMove("/source", "/target", "/backup", fs)).toContain(
      "/backup",
    );
    expect(fs.files.get("/target")).toBe("new");
    expect(fs.files.get("/backup")).toBe("original");
  });

  it("does not touch either file when the backup name is occupied", async () => {
    const fs = fakeFs();
    fs.files.set("/backup", "keep");
    await expect(
      replaceMove("/source", "/target", "/backup", fs),
    ).rejects.toThrow("destination occupied");
    expect(fs.files).toEqual(
      new Map([
        ["/source", "new"],
        ["/target", "original"],
        ["/backup", "keep"],
      ]),
    );
    expect(fs.remove).not.toHaveBeenCalled();
  });
});
