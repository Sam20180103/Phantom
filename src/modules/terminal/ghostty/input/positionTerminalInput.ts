import type { TerminalFontMetrics } from "@/modules/terminal/ghostty/gpu/terminalVisuals";

export function positionTerminalInput(
  input: HTMLTextAreaElement,
  cursor: { readonly x: number; readonly y: number },
  metrics: Pick<TerminalFontMetrics, "cellWidth" | "cellHeight">,
  cols: number,
  rows: number,
  shiftY = 0,
): void {
  // The native Copy menu temporarily owns the input's position and size.
  if (input.style.pointerEvents === "auto") return;
  const left = `${Math.max(0, Math.min(cols - 1, cursor.x)) * metrics.cellWidth}px`;
  const top = `${Math.max(0, Math.min(rows - 1, cursor.y) * metrics.cellHeight - shiftY)}px`;
  const width = `${metrics.cellWidth}px`;
  const height = `${metrics.cellHeight}px`;
  if (input.style.left !== left) input.style.left = left;
  if (input.style.top !== top) input.style.top = top;
  if (input.style.width !== width) input.style.width = width;
  if (input.style.height !== height) input.style.height = height;
  if (input.style.lineHeight !== height) input.style.lineHeight = height;
}
