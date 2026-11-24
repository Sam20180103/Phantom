export type WatchIo = {
  add: (paths: string[]) => void;
  remove: (paths: string[]) => void;
};

export function createWatchRegistry(io: WatchIo) {
  const desired = new Set<string>();
  let armed = false;
  const addMany = (paths: readonly string[]) => {
    const fresh: string[] = [];
    for (const path of paths) {
      if (desired.has(path)) continue;
      desired.add(path);
      fresh.push(path);
    }
    if (armed && fresh.length > 0) io.add(fresh);
  };
  const removeMany = (paths: readonly string[]) => {
    const gone = paths.filter((path) => desired.delete(path));
    if (armed && gone.length > 0) io.remove(gone);
  };

  return {
    add: (path: string) => addMany([path]),
    addMany,
    remove: (path: string) => removeMany([path]),
    removeMany,
    paths: () => [...desired],
    arm() {
      if (armed) return;
      armed = true;
      if (desired.size > 0) io.add([...desired]);
    },
    disarm() {
      if (!armed) return;
      armed = false;
      if (desired.size > 0) io.remove([...desired]);
    },
    clear() {
      if (armed && desired.size > 0) io.remove([...desired]);
      desired.clear();
    },
  };
}
