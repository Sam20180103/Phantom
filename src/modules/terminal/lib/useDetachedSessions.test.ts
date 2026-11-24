import { describe, expect, it } from "vitest";
import { selectDetachedSessions } from "./useDetachedSessions";

const session = (name: string, attached = false) => ({ name, attached });

describe("selectDetachedSessions", () => {
  it("offers only sessions no client is already showing", () => {
    const got = selectDetachedSessions(
      [session("phantom-1-0", true), session("phantom-2-0")],
      [],
    );
    expect(got.map((s) => s.name)).toEqual(["phantom-2-0"]);
  });

  it("hides a session this client's own (cold) tab already owns", () => {
    // A reloaded tab is cold until activated, so its session reads unattached;
    // owning it must keep the banner from offering the client its own tab back.
    const got = selectDetachedSessions(
      [session("phantom-mine"), session("phantom-other")],
      [],
      ["phantom-mine"],
    );
    expect(got.map((s) => s.name)).toEqual(["phantom-other"]);
  });

  it("never offers an agent's session", () => {
    // `pty_open` refuses `phantom-agent-*`, so a terminal tab that adopted one
    // could never reattach: every reload the refusal sent the retry off to
    // spawn a throwaway shell nothing could reach. The agent is reachable from
    // the Agents panel instead.
    const got = selectDetachedSessions(
      [session("phantom-agent-agy"), session("phantom-2-0")],
      [],
    );
    expect(got.map((s) => s.name)).toEqual(["phantom-2-0"]);
  });

  it("offers nothing when the backend reports nothing", () => {
    // The local PTY backend's shape: `listPtySessions` turns the unknown
    // command into an empty list rather than an error.
    expect(selectDetachedSessions([], [])).toEqual([]);
  });

  it("hides a dismissed session", () => {
    const got = selectDetachedSessions([session("phantom-1-0")], [
      "phantom-1-0",
    ]);
    expect(got).toEqual([]);
  });

  it("still offers a session that appears after a dismissal", () => {
    // An agent finishing overnight must not be swallowed because the banner
    // was waved off earlier in the day.
    const got = selectDetachedSessions(
      [session("phantom-1-0"), session("phantom-2-0")],
      ["phantom-1-0"],
    );
    expect(got.map((s) => s.name)).toEqual(["phantom-2-0"]);
  });

  it("offers a session that was attached when it was dismissed and has since been handed back", () => {
    // Dismissal records only what was visible, so an attached session dropping
    // its client later still surfaces.
    const got = selectDetachedSessions([session("phantom-9-0")], []);
    expect(got.map((s) => s.name)).toEqual(["phantom-9-0"]);
  });
});
