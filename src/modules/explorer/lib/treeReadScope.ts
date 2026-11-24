export function createTreeReadScope() {
  let active = false;
  const pending = new Map<string, symbol>();

  return {
    isActive: () => active,
    activate() {
      active = true;
    },
    suspend() {
      active = false;
      pending.clear();
    },
    cancelUnder(roots: readonly string[]) {
      for (const path of pending.keys()) {
        if (roots.some((root) => path === root || path.startsWith(`${root}/`))) {
          pending.delete(path);
        }
      }
    },
    begin(path: string) {
      if (!active) return null;
      const token = Symbol();
      pending.set(path, token);
      return {
        isCurrent: () => active && pending.get(path) === token,
        finish() {
          if (pending.get(path) === token) pending.delete(path);
        },
      };
    },
  };
}
