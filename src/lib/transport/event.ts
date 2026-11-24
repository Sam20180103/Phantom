/**
 * Drop-in replacement for `@tauri-apps/api/event`, wired up by a Vite alias.
 *
 * Server-originated events (agent signals, file-change notifications) arrive
 * on the same socket and are dispatched here by name. Frontend-only events
 * still round-trip locally so in-app `emit`/`listen` pairs keep working.
 */

import { transport } from "./client";

export type UnlistenFn = () => void;

export type Event<T> = {
  event: string;
  id: number;
  payload: T;
};

let nextEventId = 1;

export async function listen<T>(
  event: string,
  handler: (event: Event<T>) => void,
): Promise<UnlistenFn> {
  return transport.listen(event, (payload) => {
    handler({ event, id: nextEventId++, payload: payload as T });
  });
}

/** Fires once, then unsubscribes. */
export async function once<T>(
  event: string,
  handler: (event: Event<T>) => void,
): Promise<UnlistenFn> {
  const unlisten = await listen<T>(event, (e) => {
    unlisten();
    handler(e);
  });
  return unlisten;
}

/**
 * Local-only: delivers to listeners in this page. Nothing in Phantom needs a
 * browser-originated event to reach the server, so this deliberately does not
 * cross the socket — if that changes, add a `ClientMsg` variant rather than
 * broadening this silently.
 */
export async function emit(event: string, payload?: unknown): Promise<void> {
  transport.dispatch(event, payload);
}

export async function emitTo(
  _target: string,
  event: string,
  payload?: unknown,
): Promise<void> {
  await emit(event, payload);
}
