export type BatchMoveItem = {
  from: string;
  to: string;
  name: string;
  conflict: boolean;
};

export function batchRootPaths(paths: string[]): string[] {
  const unique = new Set(
    paths.map((path) => path.replace(/\\/g, "/").replace(/\/+$/, "") || "/"),
  );
  return [...unique].filter((path) => {
    let parent = path;
    while (parent.includes("/")) {
      parent = parent.slice(0, parent.lastIndexOf("/"));
      if (!parent && path === "/") break;
      if (unique.has(parent || "/")) return false;
    }
    return true;
  });
}

export function planBatchMove(
  sources: string[],
  toDir: string,
  existingNames: string[],
): BatchMoveItem[] {
  const roots = batchRootPaths(sources);
  const target = toDir.replace(/\\/g, "/").replace(/\/+$/, "");
  if (
    roots.some(
      (from) =>
        from === "/" || from === target || target.startsWith(`${from}/`),
    )
  ) {
    return [];
  }
  const existing = new Set(existingNames);
  const claimed = new Set<string>();
  const items: BatchMoveItem[] = [];
  for (const from of roots) {
    const name = from.slice(from.lastIndexOf("/") + 1);
    const to = `${target}/${name}`;
    if (to === from) continue;
    const conflict = existing.has(name) || claimed.has(name);
    claimed.add(name);
    items.push({ from, to, name, conflict });
  }
  return items;
}
