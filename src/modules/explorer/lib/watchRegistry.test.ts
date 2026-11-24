import { describe, expect, it, vi } from "vitest";
import { createWatchRegistry } from "@/modules/explorer/lib/watchRegistry";

function setup() {
  const io = { add: vi.fn(), remove: vi.fn() };
  return { io, registry: createWatchRegistry(io) };
}

describe("explorer watch ownership", () => {
  it("does not acquire native watches until shown", () => {
    const { io, registry } = setup();
    registry.addMany(["/a", "/b", "/a"]);
    registry.remove("/a");
    expect(io.add).not.toHaveBeenCalled();
    expect(io.remove).not.toHaveBeenCalled();
    registry.arm();
    expect(io.add).toHaveBeenCalledExactlyOnceWith(["/b"]);
  });

  it("deduplicates each batch and releases only owned paths", () => {
    const { io, registry } = setup();
    registry.arm();
    registry.addMany(["/a", "/a", "/b"]);
    registry.add("/a");
    registry.removeMany(["/a", "/a", "/unknown"]);
    expect(io.add).toHaveBeenCalledExactlyOnceWith(["/a", "/b"]);
    expect(io.remove).toHaveBeenCalledExactlyOnceWith(["/a"]);
    expect(registry.paths()).toEqual(["/b"]);
  });

  it("hides and shows idempotently while remembering the latest desired set", () => {
    const { io, registry } = setup();
    registry.addMany(["/a", "/b"]);
    registry.arm();
    registry.disarm();
    registry.disarm();
    registry.remove("/a");
    registry.add("/c");
    registry.arm();
    registry.arm();
    expect(io.add.mock.calls).toEqual([[["/a", "/b"]], [["/b", "/c"]]]);
    expect(io.remove.mock.calls).toEqual([[["/a", "/b"]]]);
  });

  it("clears desired paths without double-releasing a hidden owner", () => {
    const { io, registry } = setup();
    registry.add("/a");
    registry.arm();
    registry.disarm();
    registry.clear();
    registry.clear();
    registry.arm();
    expect(registry.paths()).toEqual([]);
    expect(io.remove).toHaveBeenCalledExactlyOnceWith(["/a"]);
    expect(io.add).toHaveBeenCalledOnce();
  });

  it("releases an armed owner on unmount", () => {
    const { io, registry } = setup();
    registry.arm();
    registry.addMany(["/a", "/b"]);
    registry.clear();
    expect(io.remove).toHaveBeenCalledExactlyOnceWith(["/a", "/b"]);
  });
});
