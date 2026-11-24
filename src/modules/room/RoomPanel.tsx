import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Conversation,
  ConversationContent,
  ConversationEmptyState,
  ConversationScrollButton,
} from "@/components/ai-elements/conversation";
import { cn } from "@/lib/utils";
import { currentWorkspaceEnv } from "@/modules/workspace";
import { useRoomStore } from "./store/roomStore";
import {
  type Attachment,
  composePost,
  MAX_ATTACHMENT_BYTES,
  MAX_ATTACHMENTS,
  participantId,
  peek,
  type RoomMsg,
  type RoomRef,
  type RosterEntry,
} from "./lib/room";

type Props = {
  /** Workspace root: locates `.phantom/agents.yaml` and `.phantom/handoff/`. */
  cwd: string | undefined;
};

type DirEntry = { name: string; kind: "file" | "dir" | "symlink" };

const ROSTER_POLL_MS = 15_000;
/**
 * After a mention, watch closely for a while: the point is to see the agent
 * pick the work up, not to hear about it a quarter of a minute later.
 */
const ROSTER_FAST_MS = 3_000;
const FAST_FOR_MS = 90_000;

/** How often an open look at an agent's screen refreshes. */
const PEEK_EVERY_MS = 3_000;

/** A file waiting for the next send, keyed so removing one can't reuse a key. */
type Pending = Attachment & { key: number };
let nextPendingKey = 1;

/**
 * The room, as a chat.
 *
 * Sits beside `AgentsPanel` rather than replacing it: that panel is the control
 * surface (open a session, dispatch one task, run the orchestrator), this one is
 * the conversation. Mentioning an agent here injects into its session, so the
 * same primitive drives both — the difference is only who is addressed and
 * whether everyone else sees it.
 */
export function RoomPanel({ cwd }: Props) {
  const messages = useRoomStore((s) => s.messages);
  const roster = useRoomStore((s) => s.roster);
  const localPeer = useRoomStore((s) => s.localPeer);
  const room = useRoomStore((s) => s.room);
  const gap = useRoomStore((s) => s.gap);
  const unavailable = useRoomStore((s) => s.unavailable);
  const hub = useRoomStore((s) => s.hub);
  const subscribe = useRoomStore((s) => s.subscribe);
  const refreshRoster = useRoomStore((s) => s.refreshRoster);
  const send = useRoomStore((s) => s.send);

  const [text, setText] = useState("");
  const [refs, setRefs] = useState<RoomRef[]>([]);
  const [handoffs, setHandoffs] = useState<string[]>([]);
  const [picking, setPicking] = useState(false);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [attached, setAttached] = useState<Pending[]>([]);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const fileRef = useRef<HTMLInputElement>(null);
  const fastUntil = useRef(0);
  // An agent's screen being looked at, instead of the conversation.
  const [peeking, setPeeking] = useState<string | null>(null);
  const [screen, setScreen] = useState<{ text: string; error?: string } | null>(
    null,
  );

  useEffect(() => {
    if (!peeking) return;
    let open = true;
    setScreen(null);
    const load = async () => {
      try {
        const text = await peek(peeking);
        if (open) setScreen({ text });
      } catch (e) {
        if (open)
          setScreen({
            text: "",
            error: e instanceof Error ? e.message : String(e),
          });
      }
    };
    void load();
    const timer = setInterval(() => void load(), PEEK_EVERY_MS);
    return () => {
      open = false;
      clearInterval(timer);
    };
  }, [peeking]);

  useEffect(() => {
    void subscribe();
  }, [subscribe]);

  // Polled while the panel is open: agents start and stop, on this machine and
  // on peers, and the dots are how you tell whether a mention will land. A
  // room on a hub lists the hub's agents, so it needs no folder open here.
  const rosterCwd = cwd ?? (hub ? "" : undefined);
  useEffect(() => {
    if (rosterCwd === undefined) return;
    let last = 0;
    const tick = () => {
      const now = Date.now();
      if (now < fastUntil.current || now - last >= ROSTER_POLL_MS) {
        last = now;
        void refreshRoster(rosterCwd);
      }
    };
    tick();
    const timer = setInterval(tick, ROSTER_FAST_MS);
    return () => clearInterval(timer);
  }, [rosterCwd, refreshRoster]);

  // The "+" menu's contents. Read on open rather than polled: a handoff file
  // appearing is rare, and the list is only ever looked at while attaching one.
  const openPicker = async () => {
    setPicking((p) => !p);
    if (picking || !cwd) return;
    try {
      const entries = await invoke<DirEntry[]>("fs_read_dir", {
        path: `${cwd}/.phantom/handoff`,
        workspace: currentWorkspaceEnv(),
      });
      setHandoffs(entries.filter((e) => e.kind === "file").map((e) => e.name));
    } catch {
      setHandoffs([]); // no handoff dir yet
    }
  };

  const submit = async () => {
    const post = composePost(
      text,
      attached.map(({ name, text }) => ({ name, text })),
    );
    if (!post || sending) return;
    if (post.attach.length > MAX_ATTACHMENTS) {
      setSendError(`at most ${MAX_ATTACHMENTS} attachments per message`);
      return;
    }
    setSending(true);
    setSendError(null);
    try {
      await send(post.text, refs, post.attach);
      setText("");
      setRefs([]);
      setAttached([]);
      if (post.text.includes("@")) fastUntil.current = Date.now() + FAST_FOR_MS;
    } catch (e) {
      // Keep the draft: a post that did not land must not take the text with it.
      setSendError(e instanceof Error ? e.message : String(e));
    } finally {
      setSending(false);
    }
  };

  const attachFiles = async (files: File[]) => {
    setSendError(null);
    const next = [...attached];
    for (const file of files) {
      if (next.length >= MAX_ATTACHMENTS) {
        setSendError(`at most ${MAX_ATTACHMENTS} attachments per message`);
        break;
      }
      if (file.size > MAX_ATTACHMENT_BYTES) {
        setSendError(`${file.name} is over 64 KiB`);
        continue;
      }
      const content = await file.text();
      // A NUL is the tell of a binary file read as text.
      if (content.includes("\0")) {
        setSendError(`${file.name} is not a text file`);
        continue;
      }
      next.push({ name: file.name, text: content, key: nextPendingKey++ });
    }
    setAttached(next);
  };

  const mentionable = useMemo(
    () => roster.filter((r) => r.agent !== null),
    [roster],
  );

  // Who is busy, and who is waiting on a person, shown under the conversation
  // — each a way to look at what that agent's screen shows.
  const inState = (...states: string[]) =>
    roster
      .filter(
        (r) => r.agent !== null && r.running && states.includes(r.state ?? ""),
      )
      .map((r) => `${r.peer}/${r.agent}`);
  const busy = inState("working", "started");
  const waiting = inState("attention");

  const insertMention = (id: string) => {
    // The bare agent name is what a local mention needs; the qualified form is
    // only required once peers exist, and the server accepts both.
    const short = id.startsWith(`${localPeer}/`) ? id.slice(localPeer.length + 1) : id;
    setText((t) => `${t}${t && !t.endsWith(" ") ? " " : ""}@${short} `);
    inputRef.current?.focus();
  };

  return (
    <div className="flex h-full min-h-0 flex-col text-sm">
      <div className="flex items-center gap-2 border-b border-border/60 px-3 py-2">
        <span className="font-medium">Room</span>
        {room && (
          <span className="truncate text-xs text-muted-foreground">#{room}</span>
        )}
        {/* Which machine's room this is matters once a client can point
            elsewhere: posting into the wrong room is silent otherwise. */}
        {hub && (
          <span className="truncate text-[11px] text-muted-foreground/70">
            @ {hub.replace(/^https?:\/\//, "")}
          </span>
        )}
        {gap && (
          <span className="text-[11px] text-muted-foreground">catching up…</span>
        )}
        <button
          type="button"
          onClick={() =>
            rosterCwd !== undefined && void refreshRoster(rosterCwd)
          }
          className="ml-auto text-xs text-muted-foreground hover:text-foreground"
        >
          Refresh
        </button>
      </div>

      {mentionable.length > 0 && (
        <div className="flex flex-wrap gap-1 border-b border-border/60 px-3 py-1.5">
          {mentionable.map((r) => (
            <button
              key={r.id}
              type="button"
              onClick={() => insertMention(r.id)}
              title={[
                r.role,
                r.peer === localPeer ? "on this machine" : `on ${r.peer}`,
                agentStatus(r).label,
              ]
                .filter(Boolean)
                .join(" · ")}
              className="flex items-center gap-1 rounded-full border border-border/60 px-2 py-0.5 text-[11px] hover:bg-foreground/5"
            >
              <span
                className={cn("h-1.5 w-1.5 rounded-full", agentStatus(r).dot)}
              />
              {/* Which machine an agent is on is not decoration: `@claude` and
                  `@b/claude` are different agents doing different work, and the
                  chip is where you pick between them. Same `peer/agent` shape
                  the message headers use, so the two read as one identity. */}
              <span className="text-muted-foreground">{r.peer}/</span>
              {r.agent}
            </button>
          ))}
        </div>
      )}

      {peeking && (
        <div className="flex min-h-0 flex-1 flex-col">
          <div className="flex items-center gap-2 border-b border-border/60 px-3 py-1.5 text-[11px] text-muted-foreground">
            <span className="truncate">
              {peeking}'s screen · read-only, refreshing
            </span>
            <button
              type="button"
              onClick={() => setPeeking(null)}
              className="ml-auto hover:text-foreground"
            >
              Close
            </button>
          </div>
          <pre className="min-h-0 flex-1 overflow-auto whitespace-pre px-3 py-2 font-mono text-[11px] leading-snug">
            {screen === null
              ? "…"
              : (screen.error ?? (screen.text || "(nothing on screen)"))}
          </pre>
        </div>
      )}

      <Conversation className={cn("min-h-0 flex-1", peeking && "hidden")}>
        <ConversationContent className="gap-3 p-3">
          {unavailable ? (
            <ConversationEmptyState
              title="No room here"
              description={unavailableReason(unavailable, hub)}
            />
          ) : messages.length === 0 ? (
            <ConversationEmptyState
              title="Nobody has said anything"
              description="Mention an agent with @name to hand it work."
            />
          ) : (
            messages.map((m) => (
              <RoomBubble key={m.seq} msg={m} localPeer={localPeer} />
            ))
          )}
        </ConversationContent>
        <ConversationScrollButton />
      </Conversation>

      {(busy.length > 0 || waiting.length > 0) && (
        <p
          role="status"
          className="px-3 pb-1 text-[11px] text-muted-foreground"
        >
          {busy.length > 0 && (
            <>
              ⏳ <PeekLinks ids={busy} onPeek={setPeeking} /> working…{" "}
            </>
          )}
          {waiting.length > 0 && (
            <>
              ❓ <PeekLinks ids={waiting} onPeek={setPeeking} /> waiting for
              someone
            </>
          )}
        </p>
      )}

      <div className="border-t border-border/60 px-3 py-2">
        {sendError && (
          <p role="alert" className="mb-1.5 text-[11px] text-destructive">
            not sent: {sendError}
          </p>
        )}
        {attached.length > 0 && (
          <div className="mb-1.5 flex flex-wrap gap-1">
            {attached.map((a) => (
              <button
                key={a.key}
                type="button"
                onClick={() =>
                  setAttached((as) => as.filter((x) => x.key !== a.key))
                }
                title="remove"
                className="rounded border border-border/60 px-1.5 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
              >
                📎 {a.name} ×
              </button>
            ))}
          </div>
        )}
        {refs.length > 0 && (
          <div className="mb-1.5 flex flex-wrap gap-1">
            {refs.map((r) => (
              <button
                key={refKey(r)}
                type="button"
                onClick={() => setRefs((rs) => rs.filter((x) => refKey(x) !== refKey(r)))}
                title="remove"
                className="rounded border border-border/60 px-1.5 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
              >
                {refLabel(r)} ×
              </button>
            ))}
          </div>
        )}
        {picking && (
          <ul className="mb-1.5 max-h-32 overflow-y-auto rounded border border-border/50 text-[11px]">
            {handoffs.length === 0 ? (
              <li className="px-2 py-1 text-muted-foreground">
                nothing in .phantom/handoff/
              </li>
            ) : (
              handoffs.map((f) => (
                <li key={f}>
                  <button
                    type="button"
                    onClick={() => {
                      const r: RoomRef = { kind: "handoff", peer: localPeer, file: f };
                      setRefs((rs) =>
                        rs.some((x) => refKey(x) === refKey(r)) ? rs : [...rs, r],
                      );
                      setPicking(false);
                    }}
                    className="w-full px-2 py-1 text-left hover:bg-foreground/5"
                  >
                    {f}
                  </button>
                </li>
              ))
            )}
          </ul>
        )}
        <div className="flex items-end gap-1.5">
          {/* Text from this device, kept as a file on the server the room
              runs on — the one way to hand over a file from a machine that
              doesn't share the room's disk. */}
          <button
            type="button"
            onClick={() => fileRef.current?.click()}
            title="attach a text file from this device"
            className="shrink-0 rounded border border-border/60 px-2 py-1 text-xs hover:bg-foreground/5"
          >
            📎
          </button>
          <input
            ref={fileRef}
            type="file"
            multiple
            hidden
            accept="text/*,.md,.txt,.log,.json,.csv,.yaml,.yml,.toml,.xml,.html,.css,.js,.ts,.py,.rs,.go,.sh,.sql"
            onChange={(e) => {
              void attachFiles([...(e.target.files ?? [])]);
              e.target.value = "";
            }}
          />
          {/* The picker lists this machine's handoff folder. A room on a hub
              would name those files as the hub's — references to files that
              are not there — so it is offered only for the local room. */}
          {!hub && (
            <button
              type="button"
              onClick={() => void openPicker()}
              title="attach a handoff file"
              className="shrink-0 rounded border border-border/60 px-2 py-1 text-xs hover:bg-foreground/5"
            >
              +
            </button>
          )}
          <textarea
            ref={inputRef}
            value={text}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.shiftKey) {
                e.preventDefault();
                void submit();
              }
            }}
            rows={1}
            placeholder="say something…  (@agent to hand it work)"
            className="min-w-0 flex-1 resize-none overflow-y-auto rounded border border-border/60 bg-transparent px-2 py-1 text-xs leading-snug outline-none focus:border-primary/50 [field-sizing:content] max-h-32"
          />
          <button
            type="button"
            disabled={(!text.trim() && attached.length === 0) || sending}
            onClick={() => void submit()}
            className="shrink-0 rounded border border-primary/45 px-2 py-1 text-xs font-medium hover:bg-primary/10 disabled:opacity-40"
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * Why the room can't be shown. Only a backend that lacks the room's commands
 * gets the "no room" explanation; a hub that can't be reached, or a dropped
 * connection, is worth saying as it is.
 */
function unavailableReason(reason: string, hub: string | null): string {
  if (hub) return `Could not reach the room at ${hub}: ${reason}`;
  if (/unknown command|not found/i.test(reason)) {
    return "This backend does not serve a room. The desktop app reaches one by pointing at a server that does.";
  }
  return reason;
}

/** Whether handing this agent work now will land, and how soon. */
function agentStatus(r: RosterEntry): { label: string; dot: string } {
  if (!r.running)
    return { label: "not running", dot: "bg-muted-foreground/40" };
  switch (r.state) {
    case "working":
    case "started":
      return { label: "working", dot: "bg-amber-500 animate-pulse" };
    case "attention":
      return { label: "waiting for someone", dot: "bg-orange-500" };
    case "exited":
      // The session is at a shell prompt: delivery refuses rather than type
      // into it, so a mention here will only come back as a notice.
      return { label: "CLI exited — start it again", dot: "bg-destructive/70" };
    default:
      return { label: "idle", dot: "bg-green-500" };
  }
}

/** Agent names that open a look at that agent's screen. */
function PeekLinks({
  ids,
  onPeek,
}: {
  ids: string[];
  onPeek: (id: string) => void;
}) {
  return ids.map((id, i) => (
    <span key={id}>
      {i > 0 && ", "}
      <button
        type="button"
        onClick={() => onPeek(id)}
        title="see what its screen shows"
        className="underline decoration-dotted underline-offset-2 hover:text-foreground"
      >
        {id}
      </button>
    </span>
  ));
}

function refKey(r: RoomRef): string {
  return r.kind === "handoff" ? `h:${r.peer}:${r.file}` : `b:${r.peer}:${r.id}`;
}

function refLabel(r: RoomRef): string {
  return r.kind === "handoff" ? r.file : `blob ${r.id.slice(0, 8)}`;
}

/**
 * Deliberately not `ai-elements/message.tsx`, whose bubble would be a perfect
 * fit: it renders markdown, so importing it pulls streamdown and CodeMirror
 * into the startup bundle (`src/app/eager-budget.test.ts` catches this). Room
 * messages are one line of plain text — a div is the cheaper answer.
 */
function RoomBubble({ msg, localPeer }: { msg: RoomMsg; localPeer: string }) {
  if (msg.kind === "notice") {
    return (
      <p className="max-w-[90%] self-center text-center text-[11px] text-muted-foreground">
        <span className="font-medium">{msg.from.peer}</span> · {msg.text}
      </p>
    );
  }
  const mine = msg.from.agent === null && msg.from.peer === localPeer;
  return (
    <div className={cn("flex flex-col gap-1", mine && "ml-auto max-w-[85%] items-end")}>
      {!mine && (
        <span className="text-[11px] font-medium text-muted-foreground">
          {participantId(msg.from)}
        </span>
      )}
      <div
        className={cn(
          "flex w-fit max-w-full flex-col gap-1 overflow-hidden text-[12px] leading-relaxed",
          mine && "rounded-2xl rounded-br-sm bg-muted/70 px-3 py-2",
        )}
      >
        <span className="whitespace-pre-wrap break-words">{msg.text}</span>
        {msg.refs.length > 0 && (
          <span className="text-[11px] text-muted-foreground">
            {msg.refs.map(refLabel).join(" · ")}
          </span>
        )}
      </div>
    </div>
  );
}
