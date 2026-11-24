import { describe, expect, it } from "vitest";
import { composePost, MAX_MESSAGE_BYTES } from "./room";

describe("composePost", () => {
  it("sends a short message as it is", () => {
    expect(composePost("  @claude hi  ", [])).toEqual({
      text: "@claude hi",
      attach: [],
    });
  });

  it("moves an over-long message into a file, keeping its first line", () => {
    // The mention is on the first line, and it only routes from the message.
    const long = `@ai what broke?\n${"ERROR boom\n".repeat(1000)}`;
    const post = composePost(long, []);
    expect(post?.text).toBe("@ai what broke? … (full text attached)");
    expect(post?.attach).toEqual([{ name: "message.txt", text: long.trim() }]);
  });

  it("counts the limit in bytes, which is what the server counts", () => {
    // Three bytes a character: over the limit well before its length says so.
    const chinese = "错".repeat(Math.ceil(MAX_MESSAGE_BYTES / 3) + 1);
    expect(chinese.length).toBeLessThan(MAX_MESSAGE_BYTES);
    expect(composePost(chinese, [])?.attach).toHaveLength(1);
  });

  it("names the files when there is no text, and sends nothing when empty", () => {
    expect(composePost("", [{ name: "a.log", text: "x" }])?.text).toBe(
      "📎 a.log",
    );
    expect(composePost("   ", [])).toBeNull();
  });
});
