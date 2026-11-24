import type { Tab } from "@/modules/tabs";
import type { TerminalSearchController } from "@/modules/terminal/search/TerminalSearchController";
import { useEffect, useMemo, useRef } from "react";
import { DetachedSessionsBanner } from "./DetachedSessionsBanner";
import type { PtySessionSummary } from "./lib/pty-bridge";
import { selectLiveTerminals } from "./lib/liveTerminals";
import { leafIds } from "./lib/panes";
import { PaneTreeView } from "./PaneTreeView";
import type { TerminalPaneHandle } from "./TerminalPane";

type Props = {
  tabs: Tab[];
  activeId: number;
  /** Register/unregister handle by leaf id (not tab id). */
  registerHandle: (leafId: number, handle: TerminalPaneHandle | null) => void;
  onSearchReady: (leafId: number, addon: TerminalSearchController) => void;
  onCwd: (leafId: number, cwd: string) => void;
  onExit: (leafId: number, code: number) => void;
  onFocusLeaf: (tabId: number, leafId: number) => void;
  /** Opens a tab that takes over the named rmux session. */
  onAttachSession: (name: string) => void;
  /** Ends the named session for good, then refreshes the list. */
  onKillSession: (name: string) => void;
  /**
   * Reattachable sessions, owned by the caller rather than fetched here: the
   * tab bar offers the same list, and two pollers would disagree.
   */
  detachedSessions: PtySessionSummary[];
  onDismissSessions: () => void;
};

type Bundle = {
  setRef: (h: TerminalPaneHandle | null) => void;
  onSearchReady: (leafId: number, addon: TerminalSearchController) => void;
  onCwd: (leafId: number, cwd: string) => void;
  onExit: (leafId: number, code: number) => void;
};

export function TerminalStack({
  tabs,
  activeId,
  registerHandle,
  onSearchReady,
  onCwd,
  onExit,
  onFocusLeaf,
  onAttachSession,
  onKillSession,
  detachedSessions,
  onDismissSessions,
}: Props) {
  const terminals = useMemo(() => selectLiveTerminals(tabs), [tabs]);

  const registerRef = useRef(registerHandle);
  const searchReadyRef = useRef(onSearchReady);
  const cwdRef = useRef(onCwd);
  const exitRef = useRef(onExit);
  useEffect(() => {
    registerRef.current = registerHandle;
  }, [registerHandle]);
  useEffect(() => {
    searchReadyRef.current = onSearchReady;
  }, [onSearchReady]);
  useEffect(() => {
    cwdRef.current = onCwd;
  }, [onCwd]);
  useEffect(() => {
    exitRef.current = onExit;
  }, [onExit]);

  const bundles = useRef(new Map<number, Bundle>());
  const getBundle = (leafId: number): Bundle => {
    let b = bundles.current.get(leafId);
    if (!b) {
      b = {
        setRef: (h) => registerRef.current(leafId, h),
        onSearchReady: (id, addon) => searchReadyRef.current(id, addon),
        onCwd: (id, cwd) => cwdRef.current(id, cwd),
        onExit: (id, code) => exitRef.current(id, code),
      };
      bundles.current.set(leafId, b);
    }
    return b;
  };

  useEffect(() => {
    const live = new Set<number>();
    for (const t of terminals)
      for (const id of leafIds(t.paneTree)) live.add(id);
    for (const id of bundles.current.keys()) {
      if (!live.has(id)) bundles.current.delete(id);
    }
  }, [terminals]);

  return (
    // Column, not an overlay: the banner must not cover terminal output, and a
    // head-mounted display has no hover to reveal what is underneath.
    <div className="flex h-full w-full flex-col">
      <DetachedSessionsBanner
        sessions={detachedSessions}
        onAttach={onAttachSession}
        onKill={onKillSession}
        onDismiss={onDismissSessions}
      />
      <div data-terminal-stack="" className="relative min-h-0 flex-1">
      {/*
        The one canvas every WebGPU pane of this window draws into (WebGpuWindowCanvas). It is
        exempt from the app zoom like the panes, so its CSS pixels are real pixels, and it clips
        the canvas to the area. Tabs and panes above it are transparent where it shows through.
      */}
      <div
        data-terminal-area=""
        aria-hidden="true"
        className="zoom-exempt pointer-events-none absolute inset-0 overflow-hidden"
      />
      {terminals.map((t) => {
        const tabVisible = t.id === activeId;
        return (
          <div
            key={t.id}
            data-terminal-tab={t.id}
            className="absolute inset-0"
            style={{
              visibility: tabVisible ? "visible" : "hidden",
              pointerEvents: tabVisible ? "auto" : "none",
            }}
            aria-hidden={!tabVisible}
          >
            <PaneTreeView
              node={t.paneTree}
              tabVisible={tabVisible}
              activeLeafId={t.activeLeafId}
              blocks={t.blocks ?? false}
              onFocusLeaf={(leafId) => onFocusLeaf(t.id, leafId)}
              getBundle={getBundle}
            />
          </div>
        );
      })}
      </div>
    </div>
  );
}
