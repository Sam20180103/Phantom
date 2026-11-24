import type { PtySessionSummary } from "./lib/pty-bridge";

type Props = {
  sessions: PtySessionSummary[];
  onAttach: (name: string) => void;
  onKill: (name: string) => void;
  onDismiss: () => void;
};

/**
 * Offers to take over terminals still running on the server.
 *
 * This is the recovery path for the case Phantom exists to serve: an agent was
 * left running on the machine, and the user comes back on a different device.
 * Nothing else in the UI reveals those sessions, so without this they are only
 * reachable by knowing to look.
 *
 * Renders nothing when there is nothing to recover — the common case — so it
 * costs no space on a small screen.
 */
export function DetachedSessionsBanner({
  sessions,
  onAttach,
  onKill,
  onDismiss,
}: Props) {
  if (sessions.length === 0) return null;

  return (
    <div
      className="mb-2 flex flex-wrap items-center gap-2 rounded-lg border border-primary/40 bg-background/80 px-3 py-2 text-xs text-foreground"
      // Announced rather than silent: on a head-mounted display this may be the
      // only hint that work is waiting.
      role="status"
    >
      <span className="text-muted-foreground">
        {sessions.length === 1
          ? "1 terminal still running"
          : `${sessions.length} terminals still running`}
      </span>
      {sessions.map((s) => (
        <span
          key={s.name}
          className="flex items-stretch overflow-hidden rounded border border-primary/45"
        >
          <button
            type="button"
            onClick={() => onAttach(s.name)}
            className="px-2 py-0.5 font-medium hover:bg-primary/10"
          >
            {s.name.replace(/^phantom-/, "")}
          </button>
          <button
            type="button"
            onClick={() => onKill(s.name)}
            // End the session instead of taking it over — the escape hatch for a
            // shell you're done with, so it stops piling up in this list.
            aria-label={`End terminal ${s.name.replace(/^phantom-/, "")}`}
            title="End this terminal"
            className="border-l border-primary/45 px-1.5 py-0.5 text-muted-foreground hover:bg-destructive/15 hover:text-foreground"
          >
            ×
          </button>
        </span>
      ))}
      <button
        type="button"
        onClick={onDismiss}
        className="ml-auto text-muted-foreground hover:text-foreground"
      >
        Dismiss
      </button>
    </div>
  );
}
