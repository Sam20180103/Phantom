import { focusLeafInput } from "@/modules/terminal/lib/terminalSessionApi";
import { usePreferencesStore } from "@/modules/settings/preferences";
import type { TerminalSearchController } from "@/modules/terminal/search/TerminalSearchController";
import { useTheme } from "@/modules/theme";
import {
  forwardRef,
  lazy,
  memo,
  Suspense,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import type { TerminalBackendKind } from "./backend/contracts";
import { resolvedTerminalBackend } from "./backend/selection";
import {
  useGhosttyTerminalSession,
  ghosttyBlockGeometry,
} from "./ghostty/useGhosttyTerminalSession";
import { ghosttyBlocks } from "./ghostty/ghosttyBlockSessions";

export type TerminalPaneHandle = {
  write: (data: string) => void;
  focus: () => void;
  getBuffer: (maxLines?: number) => string | null;
  getSelection: () => string | null;
};

export type TerminalPaneProps = {
  /** Stable identifier for this leaf (passed back through callbacks). */
  leafId: number;
  /** Tab containing this pane is on screen. */
  visible: boolean;
  /** This leaf is the active pane within its tab and receives auto-focus. */
  focused?: boolean;
  initialCwd?: string;
  attach?: string;
  session?: string;
  /** Enable command-block decorations (OSC 133) for this terminal. */
  blocks?: boolean;
  onSearchReady?: (leafId: number, addon: TerminalSearchController) => void;
  onExit?: (leafId: number, code: number) => void;
  onCwd?: (leafId: number, cwd: string) => void;
};

const TerminalAccessibleOutput = lazy(
  () => import("./ghostty/TerminalAccessibleOutput"),
);
const GhosttyBlockOverlay = lazy(() => import("./ghostty/GhosttyBlockOverlay"));

const GhosttyTerminalPane = memo(
  forwardRef<
    TerminalPaneHandle,
    TerminalPaneProps & {
      backend: Extract<TerminalBackendKind, `ghostty-${string}`>;
    }
  >(function GhosttyTerminalPane(
    {
      leafId,
      visible,
      focused = true,
      initialCwd,
      attach,
      session: sessionName,
      blocks = false,
      onSearchReady,
      onExit,
      onCwd,
      backend,
    },
    ref,
  ) {
    const screenReader = usePreferencesStore(
      (state) => state.terminalScreenReader,
    );
    const containerRef = useRef<HTMLDivElement>(null);
    const touchStartRef = useRef<{ x: number; y: number; time: number } | null>(null);
    const handleTouchStart = (e: React.TouchEvent) => {
      if (e.touches.length === 1) {
        const t = e.touches[0];
        touchStartRef.current = {
          x: t.clientX,
          y: t.clientY,
          time: Date.now(),
        };
      }
    };

    const handleTouchEnd = (e: React.TouchEvent) => {
      if (!touchStartRef.current) return;
      const start = touchStartRef.current;
      touchStartRef.current = null;
      const t = e.changedTouches[0];
      if (!t) return;
      const dx = Math.abs(t.clientX - start.x);
      const dy = Math.abs(t.clientY - start.y);
      const dt = Date.now() - start.time;
      if (dx <= 10 && dy <= 10 && dt < 400) {
        if (blocks && ghosttyBlocks(leafId)?.getMode() === "prompt") {
          focusLeafInput(leafId);
        } else {
          session.focus();
        }
      }
    };


    const down = useRef<{ x: number; y: number } | null>(null);
    const { resolvedMode, activeTheme } = useTheme();
    const session = useGhosttyTerminalSession({
      leafId,
      backend,
      container: containerRef,
      visible,
      focused,
      initialCwd,
      attach,
      session: sessionName,
      blocks,
      onSearchReady: (search) => onSearchReady?.(leafId, search),
      onExit: (code) => onExit?.(leafId, code),
      onCwd: (cwd) => onCwd?.(leafId, cwd),
    });

    useEffect(() => {
      void resolvedMode;
      void activeTheme;
      const id = requestAnimationFrame(() => session.applyTheme());
      return () => cancelAnimationFrame(id);
    }, [resolvedMode, activeTheme, session]);

    useImperativeHandle(
      ref,
      () => ({
        write: session.write,
        focus: session.focus,
        getBuffer: session.getBuffer,
        getSelection: session.getSelection,
      }),
      [session],
    );

    const isTouchDevice = typeof window !== "undefined" && ("ontouchstart" in window || navigator.maxTouchPoints > 0);
    return (
      <div
        className="zoom-exempt flex h-full w-full flex-col overflow-hidden"
        style={{
          visibility: visible ? "visible" : "hidden",
          pointerEvents: visible ? "auto" : "none",
        }}
        onPointerDownCapture={
          blocks
            ? (event) => {
                down.current =
                  event.button === 0 &&
                  event.detail === 1 &&
                  !event.shiftKey &&
                  !event.altKey &&
                  !event.metaKey &&
                  !event.ctrlKey &&
                  containerRef.current?.contains(event.target as Node)
                    ? { x: event.clientX, y: event.clientY }
                    : null;
              }
            : undefined
        }
        onPointerUp={
          blocks
            ? (event) => {
                const origin = down.current;
                down.current = null;
                const state = ghosttyBlocks(leafId);
                const geometry = ghosttyBlockGeometry(leafId);
                if (
                  origin &&
                  geometry &&
                  geometry.height > 0 &&
                  Math.hypot(
                    event.clientX - origin.x,
                    event.clientY - origin.y,
                  ) <= 4 &&
                  !state?.model?.trackedSelection?.()
                ) {
                  const row = Math.floor(
                    (event.clientY - geometry.top) / geometry.height,
                  );
                  if (state?.model && row >= 0 && row < state.model.rows)
                    state.controller?.selectAtLine(
                      state.model.bufferLineAtViewportRow(row),
                    );
                }
                if (state?.getMode() === "prompt") state.focus?.();
              }
            : undefined
        }
        data-terminal-backend={backend}
      >
        <div className="relative min-h-0 flex-1" onTouchStart={handleTouchStart} onTouchEnd={handleTouchEnd}>
        <div ref={containerRef} className="absolute inset-0" />
        {screenReader && session.model && (
          <Suspense fallback={null}>
            <TerminalAccessibleOutput
              model={session.model}
              visible={visible}
              focused={focused}
              onExit={session.focus}
            />
          </Suspense>
        )}
        {blocks && (
          <Suspense fallback={null}>
            <GhosttyBlockOverlay leafId={leafId} />
          </Suspense>
        )}
        {session.error && (
          <div
            role="alert"
            className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-3 bg-background p-6 text-center text-sm"
          >
            <strong>
              {session.error.kind === "renderer"
                ? "Unable to display terminal"
                : "Unable to start terminal"}
            </strong>
            <p className="max-w-md break-words text-muted-foreground">
              {session.error.message}
            </p>
            {session.error.kind === "renderer" && (
              <p className="text-muted-foreground">
                Your terminal session is preserved. Retry to restore its
                display.
              </p>
            )}
            <button
              type="button"
              className="rounded-md border px-3 py-1.5 hover:bg-accent focus-visible:outline-2"
              onClick={session.retry}
            >
              {session.error.kind === "renderer" ? "Retry display" : "Retry"}
            </button>
          </div>
        )}
        </div>
        {isTouchDevice && visible && (
          <div
            className="flex h-7 shrink-0 items-center justify-between border-t border-border/40 bg-card/90 px-2 text-[11px] select-none"
            onTouchStart={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-1.5 overflow-x-auto">
              <button
                type="button"
                onPointerDown={(e) => {
                  e.preventDefault();
                  session.write("\x1b");
                  session.focus();
                }}
                className="rounded bg-muted/80 px-2 py-0.5 font-mono text-[11px] text-foreground active:bg-accent"
              >
                ESC
              </button>
              <button
                type="button"
                onPointerDown={(e) => {
                  e.preventDefault();
                  session.write("\t");
                  session.focus();
                }}
                className="rounded bg-muted/80 px-2 py-0.5 font-mono text-[11px] text-foreground active:bg-accent"
              >
                TAB
              </button>
              <button
                type="button"
                onPointerDown={(e) => {
                  e.preventDefault();
                  session.write("\x03");
                  session.focus();
                }}
                className="rounded bg-destructive/15 px-2 py-0.5 font-mono text-[11px] text-destructive active:bg-destructive/30"
              >
                CTRL+C
              </button>
              <button
                type="button"
                onPointerDown={(e) => {
                  e.preventDefault();
                  session.write("\x1b[A");
                  session.focus();
                }}
                className="rounded bg-muted/80 px-2 py-0.5 font-mono text-[11px] text-foreground active:bg-accent"
              >
                ↑
              </button>
              <button
                type="button"
                onPointerDown={(e) => {
                  e.preventDefault();
                  session.write("\x1b[B");
                  session.focus();
                }}
                className="rounded bg-muted/80 px-2 py-0.5 font-mono text-[11px] text-foreground active:bg-accent"
              >
                ↓
              </button>
            </div>
            <button
              type="button"
              onPointerDown={(e) => {
                e.preventDefault();
                session.focus();
              }}
              className="rounded bg-primary/10 px-2 py-0.5 text-[11px] font-medium text-primary active:bg-primary/20"
            >
              键盘
            </button>
          </div>
        )}
      </div>
    );
  }),
);

export const TerminalPane = memo(
  forwardRef<TerminalPaneHandle, TerminalPaneProps>(
    function TerminalPane(props, ref) {
      const [backend] = useState(() =>
        usePreferencesStore.getState().terminalRenderer === "webgl"
          ? ("ghostty-webgl" as const)
          : resolvedTerminalBackend(),
      );
      return <GhosttyTerminalPane ref={ref} {...props} backend={backend} />;
    },
  ),
);
