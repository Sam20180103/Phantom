import { useCallback, useEffect, useState } from "react";
import { isAgentSession } from "./panes";
import { listPtySessions, type PtySessionSummary } from "./pty-bridge";

/**
 * The sessions worth offering: running, unclaimed, and not already waved off.
 *
 * Split out from the hook so the filtering is testable without a DOM — the
 * dismissal rule in particular, which is easy to get wrong in the direction of
 * silently hiding work.
 */
export function selectDetachedSessions(
  all: PtySessionSummary[],
  dismissed: readonly string[],
  owned: readonly string[] = [],
): PtySessionSummary[] {
  // `owned` excludes sessions this client's own tabs hold — including cold tabs
  // not yet warmed, whose sessions read as unattached but will reattach on
  // activation. Without this the banner briefly offers a client its own tabs
  // right after a reload, which looks like a leak.
  // Agent sessions are excluded even though the server lists them: they belong
  // to the Agents panel, and `pty_open` refuses their reserved name. Offering
  // one here produced a terminal tab that could never reattach — every reload
  // it asked for the refused name and the retry spawned a throwaway shell that
  // nothing could ever reach again.
  return all.filter(
    (s) =>
      !s.attached &&
      !isAgentSession(s.name) &&
      !dismissed.includes(s.name) &&
      !owned.includes(s.name),
  );
}

/**
 * Terminals still running on the server that this client isn't showing.
 *
 * The list is intentionally *detached-only*: a session this browser already has
 * a tab for is not something to offer reattaching to, and surfacing it would
 * make the common case (one device, nothing to recover) look like there is
 * something to act on.
 *
 * Empty on the local PTY backend, where nothing outlives the socket.
 */
export function useDetachedSessions(owned: readonly string[] = []): {
  sessions: PtySessionSummary[];
  refresh: () => void;
  dismiss: () => void;
} {
  const [sessions, setSessions] = useState<PtySessionSummary[]>([]);
  // By name, not a single flag: dismissing the banner means "not these", so a
  // session that appears later still gets offered.
  const [dismissed, setDismissed] = useState<string[]>([]);

  const refresh = useCallback(() => {
    void listPtySessions().then(setSessions);
  }, []);

  // Polled rather than pushed: sessions change when a *different* device
  // attaches or a shell exits, neither of which this connection hears about.
  // A slow poll is enough — this drives an offer to reattach, not live state.
  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 15_000);
    return () => clearInterval(timer);
  }, [refresh]);

  const visible = selectDetachedSessions(sessions, dismissed, owned);
  return {
    sessions: visible,
    refresh,
    // Only the names actually on screen, and additive: dismissing must not
    // suppress a session that was attached at the time and is handed back
    // later, nor one that appears afterwards.
    dismiss: () =>
      setDismissed((prev) => [...prev, ...visible.map((s) => s.name)]),
  };
}
