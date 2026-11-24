import type { Tab } from "@/modules/tabs";
import { hasLeaf } from "@/modules/terminal/lib/panes";
import { redactSensitive } from "@/modules/ai/lib/redact";

export function readTerminalContext(
  tabs: readonly Tab[],
  leafId: number,
  readBuffer: (leafId: number, maxLines: number) => string | null | undefined,
): string | null {
  const owners = tabs.filter(
    (tab) => tab.kind === "terminal" && hasLeaf(tab.paneTree, leafId),
  );
  if (owners.length !== 1 || owners[0].kind !== "terminal" || owners[0].private)
    return null;
  const buffer = readBuffer(leafId, 300);
  return buffer ? redactSensitive(buffer) : null;
}
