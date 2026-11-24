import type { EditorPaneHandle } from "@/modules/editor/EditorPane";
import { useEditorFileSync } from "@/modules/editor/useEditorFileSync";
import type { Tab } from "@/modules/tabs";
import type { EffectCallback } from "react";
import { expect, it, vi } from "vitest";

const harness = vi.hoisted(() => ({
  effects: [] as EffectCallback[],
  add: vi.fn(),
  remove: vi.fn(),
  written: null as null | ((event: { payload: { path: string } }) => void),
  changed: null as null | ((paths: string[]) => void),
  stopWritten: vi.fn(),
  stopChanged: vi.fn(),
}));
vi.mock("react", () => ({
  useRef: (current: unknown) => ({ current }),
  useEffect: (effect: EffectCallback) => harness.effects.push(effect),
}));
vi.mock("@tauri-apps/api/webviewWindow", () => ({
  getCurrentWebviewWindow: () => ({
    listen: (_event: string, handler: typeof harness.written) => {
      harness.written = handler;
      return Promise.resolve(harness.stopWritten);
    },
  }),
}));
vi.mock("@/modules/explorer/lib/watch", () => ({
  parentDir: (path: string) => path.slice(0, path.lastIndexOf("/")),
  watchAdd: harness.add,
  watchRemove: harness.remove,
  listenFsChanged: (handler: typeof harness.changed) => {
    harness.changed = handler;
    return Promise.resolve(harness.stopChanged);
  },
}));

it("releases shared directory ownership and ignores events after unmount while listeners resolve", async () => {
  const tabs: Tab[] = [1, 2].map((id) => ({
    id,
    kind: "editor",
    spaceId: "s",
    title: `${id}.txt`,
    path: `/repo/${id}.txt`,
    dirty: false,
    preview: false,
  }));
  const reload = vi.fn(() => true);
  const editor: EditorPaneHandle = {
    save: vi.fn().mockResolvedValue(undefined),
    reload,
    setQuery: vi.fn(),
    findNext: vi.fn(),
    findPrevious: vi.fn(),
    clearQuery: vi.fn(),
    openSearch: vi.fn(),
    focus: vi.fn(),
    getSelection: () => null,
    getPath: () => "/repo/1.txt",
    gotoLine: vi.fn(),
    undo: vi.fn(),
    redo: vi.fn(),
    triggerAiComplete: vi.fn(),
    triggerCodeComplete: vi.fn(),
  };
  useEditorFileSync({
    tabs,
    tabsRef: { current: tabs },
    editorRefs: { current: new Map([[1, editor]]) },
  });
  const cleanups = harness.effects.map((effect) => effect());
  expect(harness.add).toHaveBeenCalledWith(
    ["/repo"],
    ["/repo/1.txt", "/repo/2.txt"],
  );
  harness.written?.({ payload: { path: "/repo/1.txt" } });
  expect(reload).toHaveBeenCalledOnce();
  for (const cleanup of cleanups) cleanup?.();
  expect(harness.remove).toHaveBeenLastCalledWith(
    ["/repo"],
    ["/repo/1.txt", "/repo/2.txt"],
  );
  harness.written?.({ payload: { path: "/repo/1.txt" } });
  harness.changed?.(["/repo/1.txt"]);
  expect(reload).toHaveBeenCalledOnce();
  await Promise.resolve();
  expect(harness.stopWritten).toHaveBeenCalledOnce();
  expect(harness.stopChanged).toHaveBeenCalledOnce();
});
