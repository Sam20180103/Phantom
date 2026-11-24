import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

// The phone page (`GET /room`) is plain HTML the server embeds, outside this
// app's build: nothing else would notice its script failing to parse, and a
// script that doesn't parse is a blank page on every phone.
const page = readFileSync(
  resolve(__dirname, "../../../../crates/phantom-server/src/room/page.html"),
  "utf8",
);

describe("room page", () => {
  it("has a script that parses", () => {
    const script = page.match(/<script>([\s\S]*)<\/script>/)?.[1];
    expect(script).toBeTruthy();
    // Parse only: the Function is never called.
    expect(() => new Function(script as string)).not.toThrow();
  });
});
