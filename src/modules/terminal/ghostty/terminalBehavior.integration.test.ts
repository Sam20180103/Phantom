import { readFile } from "node:fs/promises";
import { TeraxGhostty } from "@terax/ghostty-core/adapted";
import { beforeAll, describe, expect, it, vi } from "vitest";
import { AdaptedGhosttyTerminalModel } from "@/modules/terminal/ghostty/AdaptedGhosttyTerminalModel";
import { GhosttyBlocks } from "@/modules/terminal/ghostty/GhosttyBlocks";
import { GhosttySemanticEventRouter } from "@/modules/terminal/ghostty/core/GhosttySemanticEventRouter";
import { GhosttySearchController } from "@/modules/terminal/ghostty/search/GhosttySearchController";
import { isExternalUrl } from "@/lib/external-link";
import { readTerminalContext } from "@/modules/ai/lib/terminalContext";

const encoder = new TextEncoder();
const osc = (value: string) => `\x1b]${value}\x1b\\`;

describe.each(["ghostty-vt", "ghostty-vt-scalar"])(
  "product terminal behavior with real %s WASM",
  (artifact) => {
    let core: TeraxGhostty;
    beforeAll(async () => {
      const bytes = await readFile(
        new URL(
          `../../../../packages/ghostty-core/adapted/${artifact}.wasm`,
          import.meta.url,
        ),
      );
      core = await TeraxGhostty.loadBytes(Uint8Array.from(bytes).buffer);
    });

    describe.each(["ghostty-webgpu", "ghostty-webgl"] as const)(
      "%s model contract without a rendering surface",
      (backend) => {
        function create() {
          let cwd = "/initial";
          const onCwd = vi.fn((value: string) => {
            cwd = value;
          });
          const onClipboard = vi.fn();
          const router = new GhosttySemanticEventRouter({ onCwd, onClipboard });
          let blocks: GhosttyBlocks;
          const model = new AdaptedGhosttyTerminalModel(core, {
            backend,
            cols: 80,
            rows: 8,
            onEvent: (event) => {
              router.handle(event);
              blocks.handle(event, cwd);
            },
          });
          blocks = new GhosttyBlocks(model);
          return {
            model,
            blocks,
            onCwd,
            onClipboard,
            write(text: string, chunkSize = 7) {
              const bytes = encoder.encode(text);
              for (let i = 0; i < bytes.length; i += chunkSize)
                model.write(bytes.subarray(i, i + chunkSize));
            },
            dispose() {
              blocks.dispose();
              model.dispose();
            },
          };
        }

        it("routes fragmented OSC 7/52/133 into cwd, clipboard and command metadata together", () => {
          const h = create();
          try {
            h.write(
              `${osc("7;file://localhost/work/%E4%B8%AD%E6%96%87")}${osc("133;A")}$ ${osc("133;B")}`,
              1,
            );
            h.blocks.submitted("printf 中文");
            h.write(
              `${osc("133;C;printf 中文")}中文 output\r\n${osc("7;file://localhost/untrusted-output")}${osc("52;c;5Lit5paH")}${osc("133;D;7")}`,
              1,
            );
            expect(h.onCwd.mock.calls).toEqual([["/work/中文"]]);
            expect(h.onClipboard.mock.calls).toEqual([["中文"]]);
            expect(h.blocks.readById("1")).toEqual({
              command: "printf 中文",
              cwd: "/work/中文",
              exitCode: 7,
              output: "中文 output",
            });
            expect(h.blocks.visibleBlocks(16).blocks[0]).toMatchObject({
              canRerun: true,
              running: false,
              ok: false,
            });
            h.write(osc("7;file://localhost/work/next") + osc("133;A"));
            expect(h.onCwd).toHaveBeenLastCalledWith("/work/next");
          } finally {
            h.dispose();
          }
        });

        it("never turns OSC 52 queries, non-clipboard selections or invalid payloads into writes", () => {
          const h = create();
          try {
            for (const payload of ["c;?", "p;aGVsbG8=", "c;%%%", "c;/w=="])
              h.write(osc(`52;${payload}`), 1);
            expect(h.onClipboard).not.toHaveBeenCalled();
            h.write(osc("52;c;aGVsbG8="), 1);
            expect(h.onClipboard).toHaveBeenCalledExactlyOnceWith("hello");
          } finally {
            h.dispose();
          }
        });

        it("keeps Unicode selections and AI-readable history when presentation resources are released", () => {
          const h = create();
          try {
            h.write("hello 中文 e\u0301\r\nAPI_KEY=super-secret\r\n", 1);
            h.model.setSelection({
              anchor: { line: 0, column: 6 },
              focus: { line: 0, column: 9 },
              rectangular: false,
            });
            const selection = h.model.trackedSelection();
            if (!selection) throw new Error("expected a Unicode selection");
            expect(h.model.selectionText(selection)).toBe("中文");
            h.model.releasePresentationResources();
            h.write("hidden output\r\n", 1);
            const retained = h.model.trackedSelection();
            if (!retained)
              throw new Error("selection was lost during presentation release");
            expect(h.model.selectionText(retained)).toBe("中文");
            const tabs = [
              {
                id: 1,
                kind: "terminal" as const,
                title: "shell",
                spaceId: "test",
                activeLeafId: 10,
                paneTree: { kind: "leaf" as const, id: 10 },
              },
            ];
            const context = readTerminalContext(tabs, 10, (_id, lines) =>
              h.model.readText(lines),
            );
            expect(context).toContain("hello 中文 e\u0301");
            expect(context).toContain("hidden output");
            expect(context).toContain("API_KEY=<REDACTED>");
            expect(context).not.toContain("super-secret");
            expect(
              readTerminalContext([{ ...tabs[0], private: true }], 10, () =>
                h.model.readText(300),
              ),
            ).toBeNull();
          } finally {
            h.dispose();
          }
        });

        it("searches history incrementally, restores a controller and does not change grid selection", () => {
          const h = create();
          const callbacks = new Map<number, () => void>();
          let next = 0;
          const scheduler = {
            request: (callback: () => void) => {
              callbacks.set(++next, callback);
              return next;
            },
            cancel: (id: number) => {
              callbacks.delete(id);
            },
          };
          let search = new GhosttySearchController(
            h.model,
            () => {},
            scheduler,
          );
          const finish = () => {
            for (let steps = 0; callbacks.size > 0; steps++) {
              if (steps >= 100)
                throw new Error(
                  "search did not complete within its step budget",
                );
              const pending = [...callbacks.values()];
              callbacks.clear();
              for (const callback of pending) callback();
            }
          };
          try {
            h.write(
              `needle first\r\n${"ordinary\r\n".repeat(300)}中文 needle last\r\n`,
            );
            h.model.setSelection({
              anchor: { line: 301, column: 0 },
              focus: { line: 301, column: 3 },
              rectangular: false,
            });
            const selection = h.model.trackedSelection();
            search.findNext("needle");
            finish();
            const status = h.model.stepSearch();
            expect(status).toMatchObject({ complete: true, totalMatches: 2 });
            expect(status.selectedIndex).toBeGreaterThanOrEqual(0);
            const snapshot = search.snapshot();
            search.dispose();
            h.model.releasePresentationResources();
            search = new GhosttySearchController(h.model, () => {}, scheduler);
            search.restore(snapshot);
            finish();
            expect(h.model.stepSearch().selectedIndex).toBe(
              status.selectedIndex,
            );
            search.findPrevious("needle");
            finish();
            expect(h.model.stepSearch().selectedIndex).not.toBe(
              status.selectedIndex,
            );
            expect(h.model.trackedSelection()).toEqual(selection);
            search.clearDecorations();
            expect(search.viewportMaskBytes()).toBeNull();
            expect(h.model.trackedSelection()).toEqual(selection);
          } finally {
            search.dispose();
            h.dispose();
          }
        });

        it("resolves plain and OSC 8 links while unsafe schemes remain non-openable", () => {
          const h = create();
          try {
            h.write(
              `https://example.com/plain\r\n${osc("8;;https://example.com/linked")}中文链接${osc("8;;")}\r\n${osc("8;;file:///etc/passwd")}unsafe${osc("8;;")}`,
              1,
            );
            expect(h.model.hyperlinkAtViewportCell(0, 5)).toBe(
              "https://example.com/plain",
            );
            expect(h.model.hyperlinkAtViewportCell(1, 0)).toBe(
              "https://example.com/linked",
            );
            const unsafe = h.model.hyperlinkAtViewportCell(2, 0);
            if (!unsafe) throw new Error("expected the raw OSC 8 URI");
            expect(unsafe).toBe("file:///etc/passwd");
            expect(isExternalUrl(unsafe)).toBe(false);
            h.model.releasePresentationResources();
            expect(h.model.hyperlinkAtViewportCell(1, 2)).toBe(
              "https://example.com/linked",
            );
          } finally {
            h.dispose();
          }
        });

        it("keeps completed blocks across alternate-screen output and excludes TUI text from them", () => {
          const h = create();
          try {
            h.write(`${osc("133;C;echo")}normal output\r\n${osc("133;D;0")}`);
            const before = h.blocks.readById("1");
            h.write("\x1b[?1049h\x1b[2JTUI 中文");
            expect(h.blocks.mode).toBe("alt");
            expect(h.blocks.readById("1")).toBeNull();
            h.write("\x1b[?1049l");
            expect(h.blocks.readById("1")).toEqual(before);
            expect(h.blocks.readById("1")?.output).not.toContain("TUI");
          } finally {
            h.dispose();
          }
        });
      },
    );
  },
);
