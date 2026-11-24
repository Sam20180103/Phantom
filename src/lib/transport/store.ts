/**
 * Browser-safe stand-in for `@tauri-apps/plugin-store`.
 *
 * Terax persists settings, AI sessions, agents, and snippets through this. The
 * real plugin writes JSON files next to the desktop app; here we back it with
 * `localStorage`, keyed by the store's filename.
 *
 * Consequence worth knowing: state is **per browser, per device** — open
 * Phantom from a phone and a laptop and they keep separate settings. Moving
 * this to server-side storage is what makes one workspace follow you across
 * devices; see the implementation plan §4.2.
 */

const PREFIX = "phantom:store:";

type Entries = Record<string, unknown>;

function read(path: string): Entries {
  try {
    const raw = localStorage.getItem(PREFIX + path);
    if (!raw) return {};
    const parsed: unknown = JSON.parse(raw);
    // Guard against a hand-edited or partially written value: callers iterate
    // `entries()`, and a non-object here would throw far from the cause.
    return parsed && typeof parsed === "object" && !Array.isArray(parsed)
      ? (parsed as Entries)
      : {};
  } catch {
    return {};
  }
}

function write(path: string, data: Entries): void {
  try {
    localStorage.setItem(PREFIX + path, JSON.stringify(data));
  } catch (e) {
    // Quota exceeded, or storage disabled (private mode). Losing a setting is
    // better than breaking the caller.
    console.warn(`phantom: could not persist store ${path}:`, e);
  }
}

export class Store {
  #data: Entries;

  constructor(readonly path: string) {
    this.#data = read(path);
  }

  async get<T>(key: string): Promise<T | null> {
    return (this.#data[key] as T) ?? null;
  }

  async set(key: string, value: unknown): Promise<void> {
    this.#data[key] = value;
    write(this.path, this.#data);
  }

  async has(key: string): Promise<boolean> {
    return key in this.#data;
  }

  async delete(key: string): Promise<boolean> {
    const had = key in this.#data;
    delete this.#data[key];
    write(this.path, this.#data);
    return had;
  }

  async clear(): Promise<void> {
    this.#data = {};
    write(this.path, this.#data);
  }

  async keys(): Promise<string[]> {
    return Object.keys(this.#data);
  }

  async values<T>(): Promise<T[]> {
    return Object.values(this.#data) as T[];
  }

  async entries<T>(): Promise<[string, T][]> {
    return Object.entries(this.#data) as [string, T][];
  }

  async length(): Promise<number> {
    return Object.keys(this.#data).length;
  }

  /** Writes are synchronous here, so there is nothing to flush. */
  async save(): Promise<void> {}

  async reload(): Promise<void> {
    this.#data = read(this.path);
  }

  async close(): Promise<void> {}

  async onKeyChange(): Promise<() => void> {
    return () => {};
  }

  async onChange(): Promise<() => void> {
    return () => {};
  }
}

/** The real `LazyStore` defers file IO until first use; localStorage is sync. */
export class LazyStore extends Store {}

export async function load(path: string): Promise<Store> {
  return new Store(path);
}

export async function getStore(path: string): Promise<Store | null> {
  return new Store(path);
}
