import { describe, expect, it } from "vitest";
import { CLOSE_FLUSH_TIMEOUT_MS, closeAfterFlush } from "./useAppCloseGuard";

describe("closeAfterFlush", () => {
  it("closes straight away when chat history was saved", () => {
    expect(closeAfterFlush({ ok: true }, false)).toBe("close");
  });

  it("keeps the window open once, with an error, when saving failed", () => {
    expect(closeAfterFlush({ ok: false }, false)).toBe("warn");
  });

  it("never traps the user: after the warning every close request closes", () => {
    expect(closeAfterFlush({ ok: false }, true)).toBe("close");
  });

  it("bounds how long closing waits for the disk", () => {
    expect(CLOSE_FLUSH_TIMEOUT_MS).toBeGreaterThan(0);
    expect(CLOSE_FLUSH_TIMEOUT_MS).toBeLessThanOrEqual(5_000);
  });
});
