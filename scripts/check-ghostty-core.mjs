import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

const artifacts = [
  ["ghostty-vt.wasm", "302b7952e0f84d1701b64a4348f597bbe5ab2bc1f2b58c12d6cf775251a895f7"],
  ["ghostty-vt-scalar.wasm", "c9cffe386b771bd1a76b493bf064b30abb30fd5ef3d36d700226b318309c1cad"],
];

for (const [name, checksum] of artifacts) {
  const bytes = await readFile(new URL(`../packages/ghostty-core/adapted/${name}`, import.meta.url));
  assert.equal(createHash("sha256").update(bytes).digest("hex"), checksum, name);
  const { instance } = await WebAssembly.instantiate(bytes);
  const api = instance.exports;
  const terminal = api.restty_create_with_limits(12, 3, 1024 * 1024, 100);
  assert.ok(terminal, `${name}: create terminal`);
  const pointer = api.restty_alloc(1);
  assert.ok(pointer, `${name}: allocate input`);
  const write = (text) => {
    for (const byte of new TextEncoder().encode(text)) {
      new Uint8Array(api.memory.buffer)[pointer] = byte;
      assert.equal(api.restty_write(terminal, pointer, 1), 0);
    }
  };
  const firstCells = () => {
    assert.equal(api.restty_render_update(terminal), 0);
    return [...new Uint32Array(api.memory.buffer, api.restty_cell_codepoints_ptr(terminal), 4)];
  };
  try {
    write("A\x1b[31m中B\x1b[0m");
    const expected = [65, 0x4e2d, 0, 66];
    assert.deepEqual(firstCells(), expected, `${name}: split UTF-8 and ANSI`);
    write("\x1b[6n");
    const reply = new Uint8Array(api.memory.buffer, api.restty_output_ptr(terminal), api.restty_output_len(terminal));
    assert.equal(new TextDecoder().decode(reply), "\x1b[1;5R", `${name}: cursor reply`);
    write("\x1b[?1049halt\x1b[?1049l");
    assert.deepEqual(firstCells(), expected, `${name}: alternate screen restore`);
    assert.equal(api.restty_resize(terminal, 20, 5), 0);
    assert.equal(api.restty_cols(terminal), 20);
    assert.equal(api.restty_rows(terminal), 5);
    assert.deepEqual(firstCells(), expected, `${name}: resize preserves text`);
  } finally {
    api.restty_free(pointer, 1);
    api.restty_destroy(terminal);
  }
  console.log(`${name}: checksum, split UTF-8/ANSI, cursor reply, TUI restore, resize passed`);
}
