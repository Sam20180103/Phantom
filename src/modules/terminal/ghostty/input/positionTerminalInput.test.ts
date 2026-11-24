import { describe, expect, it, vi } from "vitest";
import { positionTerminalInput } from "@/modules/terminal/ghostty/input/positionTerminalInput";

describe("terminal IME anchor", () => {
  it("uses CSS cell coordinates and compensates for a shifted grid without changing composition", () => {
    const input = { style: {}, value: "中文", selectionStart: 0, selectionEnd: 2 };
    positionTerminalInput(input as HTMLTextAreaElement, { x: 10, y: 20 }, { cellWidth: 8.5, cellHeight: 18 }, 80, 24, 40);
    expect(input.style).toMatchObject({ left: "85px", top: "320px", width: "8.5px", height: "18px", lineHeight: "18px" });
    expect(input.value).toBe("中文");
    expect(input.selectionEnd).toBe(2);
  });

  it("keeps the anchor within the grid", () => {
    const input = { style: {} };
    positionTerminalInput(input as HTMLTextAreaElement, { x: 100, y: -1 }, { cellWidth: 8, cellHeight: 16 }, 80, 24, 40);
    expect(input.style).toMatchObject({ left: "632px", top: "0px" });
  });

  it("does not move an input exposed for the native Copy menu", () => {
    const style = { pointerEvents: "auto", left: "40px", top: "50px", width: "20px", height: "20px" };
    const input = { style };
    positionTerminalInput(input as HTMLTextAreaElement, { x: 10, y: 20 }, { cellWidth: 8, cellHeight: 16 }, 80, 24);
    expect(style).toEqual({ pointerEvents: "auto", left: "40px", top: "50px", width: "20px", height: "20px" });
  });

  it("does not write styles again when the cursor is unchanged", () => {
    const write = vi.fn();
    const input = { style: new Proxy({} as Record<string, string>, {
      set(target, key: string, value: string) { write(); target[key] = value; return true; },
    }) };
    const cursor = { x: 10, y: 20 };
    const metrics = { cellWidth: 8, cellHeight: 16 };
    positionTerminalInput(input as unknown as HTMLTextAreaElement, cursor, metrics, 80, 24);
    write.mockClear();
    positionTerminalInput(input as unknown as HTMLTextAreaElement, cursor, metrics, 80, 24);
    expect(write).not.toHaveBeenCalled();
  });
});
