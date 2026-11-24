import { useState } from "react";
import { usePreferencesStore } from "@/modules/settings/preferences";
import { KEYRING_SERVICE } from "@/modules/ai/config";
import { compatKeyringAccount } from "@/modules/ai/lib/keyring";
import {
  agentSessionName,
  dispatchAgent,
  ensureAgent,
  nudgeAgent,
  orchestrate,
  stopOrchestration,
  type OrchestrationAgent,
  type OrchestrationStep,
} from "./lib/orchestration";
import { useOrchestrationAgents } from "./lib/useOrchestrationAgents";

/** OpenAI-compatible providers expose chat at `<baseURL>/chat/completions`. */
function chatCompletionsUrl(baseURL: string): string {
  const trimmed = baseURL.replace(/\/+$/, "");
  return trimmed.endsWith("/chat/completions")
    ? trimmed
    : `${trimmed}/chat/completions`;
}

type Props = {
  /** Workspace root that holds `.phantom/agents.yaml`. */
  cwd: string | undefined;
  /** Opens a terminal tab attached to the given session name. */
  onAttachSession: (name: string) => void;
};

/** The canned pointer a forward injects — content itself travels by file. */
function forwardMessage(from: string): string {
  return `读 .phantom/handoff/ 里 ${from} 写的，处理一下`;
}

/**
 * Manual control surface for the multi-CLI orchestration.
 *
 * Lists the agents from `.phantom/agents.yaml` with their role and whether a
 * session is live, and exposes the three primitives by hand: open (attach a
 * terminal to the agent), dispatch (send it a task), and forward (nudge one
 * agent to pick up what another wrote to `handoff/`). The same calls are what
 * an external orchestrator will drive later — this panel is just "who presses
 * the button" today.
 */
export function AgentsPanel({ cwd, onAttachSession }: Props) {
  const { agents, refresh } = useOrchestrationAgents(cwd);
  const endpoints = usePreferencesStore((s) => s.customEndpoints);
  const [tasks, setTasks] = useState<Record<string, string>>({});
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [busy, setBusy] = useState(false);
  const [goal, setGoal] = useState("");
  const [brainId, setBrainId] = useState("");
  const [steps, setSteps] = useState<OrchestrationStep[]>([]);
  const [running, setRunning] = useState(false);

  // Default the brain to the first configured provider so the common
  // single-provider case needs no picking.
  const brainEp =
    endpoints.find((e) => e.id === brainId) ?? endpoints[0];

  const runOrchestrator = async () => {
    const g = goal.trim();
    if (!g || !cwd || running) return;
    setSteps([]);
    setRunning(true);
    try {
      const brain = brainEp
        ? {
            endpoint: chatCompletionsUrl(brainEp.baseURL),
            model: brainEp.modelId,
            keyService: KEYRING_SERVICE,
            keyAccount: compatKeyringAccount(brainEp.id),
          }
        : undefined;
      const r = await orchestrate(
        g,
        cwd,
        (step) => setSteps((s) => [...s, step]),
        brain,
      );
      setSteps((s) => [...s, { kind: "note", text: `— ${r.status} —` }]);
    } catch (e) {
      setSteps((s) => [
        ...s,
        { kind: "note", text: `error: ${(e as Error).message}` },
      ]);
    } finally {
      setRunning(false);
      refresh();
    }
  };

  const open = async (name: string) => {
    if (!cwd) return;
    await ensureAgent(name, 80, 24, cwd);
    onAttachSession(agentSessionName(name));
    refresh();
  };

  const dispatch = async (name: string) => {
    const task = (tasks[name] ?? "").trim();
    if (!task) return;
    await dispatchAgent(name, task);
    setTasks((t) => ({ ...t, [name]: "" }));
  };

  const forward = async () => {
    if (!from || !to || from === to) return;
    setBusy(true);
    try {
      await nudgeAgent(to, forwardMessage(from));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-col text-sm">
      <div className="flex items-center justify-between border-b border-border/60 px-3 py-2">
        <span className="font-medium">Agents</span>
        <button
          type="button"
          onClick={refresh}
          className="text-xs text-muted-foreground hover:text-foreground"
        >
          Refresh
        </button>
      </div>

      <div className="border-b border-border/60 px-3 py-2">
        <textarea
          value={goal}
          onChange={(e) => setGoal(e.target.value)}
          onKeyDown={(e) => {
            // Enter runs; Shift+Enter inserts a newline (multi-line goals).
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              void runOrchestrator();
            }
          }}
          disabled={running}
          rows={2}
          placeholder="orchestrate a goal…  (Enter to run · Shift+Enter for newline)"
          // field-sizing grows the box with its content up to max-h, then it
          // scrolls — a rich-text-ish input without a JS resize handler.
          className="w-full resize-none overflow-y-auto rounded border border-border/60 bg-transparent px-2 py-1 text-xs leading-snug outline-none focus:border-primary/50 disabled:opacity-60 [field-sizing:content] min-h-[3.5rem] max-h-48"
        />
        <div className="mt-1.5 flex items-center gap-1.5 text-[11px] text-muted-foreground">
          <span className="shrink-0">brain</span>
          {endpoints.length === 0 ? (
            <span className="italic">
              none — add a provider in Settings › Models
            </span>
          ) : (
            <select
              value={brainEp?.id ?? ""}
              onChange={(e) => setBrainId(e.target.value)}
              disabled={running}
              className="min-w-0 flex-1 rounded border border-border/60 bg-background px-1.5 py-1 outline-none focus:border-primary/50"
            >
              {endpoints.map((e) => (
                <option key={e.id} value={e.id}>
                  {e.name} ({e.modelId})
                </option>
              ))}
            </select>
          )}
          {running ? (
            <button
              type="button"
              onClick={() => void stopOrchestration()}
              className="shrink-0 rounded border border-destructive/50 px-2 py-1 font-medium text-destructive hover:bg-destructive/10"
            >
              Stop
            </button>
          ) : (
            <button
              type="button"
              disabled={!goal.trim() || !cwd}
              onClick={() => void runOrchestrator()}
              className="shrink-0 rounded border border-primary/45 px-2 py-1 font-medium text-foreground hover:bg-primary/10 disabled:opacity-40"
            >
              Run
            </button>
          )}
        </div>
        {steps.length > 0 && (
          <ul className="mt-1.5 max-h-40 overflow-y-auto rounded border border-border/50 bg-background/60 px-2 py-1 text-[11px] leading-snug">
            {steps.map((s, i) => (
              <li key={i} className="py-0.5">
                <StepLine step={s} />
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 py-2">
        {agents.length === 0 ? (
          <p className="px-1 py-6 text-center text-xs text-muted-foreground">
            No agents. Add them to{" "}
            <code className="rounded bg-muted px-1">.phantom/agents.yaml</code>.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {agents.map((a) => (
              <AgentCard
                key={a.name}
                agent={a}
                task={tasks[a.name] ?? ""}
                onTask={(v) => setTasks((t) => ({ ...t, [a.name]: v }))}
                onOpen={() => void open(a.name)}
                onDispatch={() => void dispatch(a.name)}
              />
            ))}
          </ul>
        )}
      </div>

      {agents.length >= 2 && (
        <div className="border-t border-border/60 px-3 py-2">
          <div className="mb-1.5 text-xs font-medium text-muted-foreground">
            Forward
          </div>
          <div className="flex items-center gap-1.5">
            <AgentSelect
              value={from}
              onChange={setFrom}
              placeholder="from"
              agents={agents}
            />
            <span className="text-muted-foreground">→</span>
            <AgentSelect
              value={to}
              onChange={setTo}
              placeholder="to"
              agents={agents}
            />
            <button
              type="button"
              disabled={busy || !from || !to || from === to}
              onClick={() => void forward()}
              className="ml-auto rounded border border-primary/45 px-2 py-1 text-xs font-medium hover:bg-primary/10 disabled:opacity-40"
            >
              Forward
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

function StepLine({ step }: { step: OrchestrationStep }) {
  const tag = (label: string, cls: string) => (
    <span className={"mr-1 font-medium " + cls}>{label}</span>
  );
  switch (step.kind) {
    case "dispatch":
      return (
        <span>
          {tag("→ dispatch", "text-primary")}
          <b>{step.agent}</b>: {step.task}
        </span>
      );
    case "nudge":
      return (
        <span>
          {tag("↷ nudge", "text-primary")}
          <b>{step.agent}</b>: {step.message}
        </span>
      );
    case "read":
      return (
        <span>
          {tag("read", "text-muted-foreground")}
          {step.file}
        </span>
      );
    case "list":
      return <span>{tag("list agents", "text-muted-foreground")}</span>;
    case "finish":
      return (
        <span>
          {tag("✓ finish", "text-green-500")}
          {step.summary}
        </span>
      );
    case "stopped":
      return <span className="text-destructive">■ stopped</span>;
    case "note":
      return <span className="text-muted-foreground">{step.text}</span>;
  }
}

function AgentCard({
  agent,
  task,
  onTask,
  onOpen,
  onDispatch,
}: {
  agent: OrchestrationAgent;
  task: string;
  onTask: (v: string) => void;
  onOpen: () => void;
  onDispatch: () => void;
}) {
  return (
    <li className="rounded-lg border border-border/60 bg-background/60 px-2.5 py-2">
      <div className="flex items-center gap-2">
        <span
          className={
            "h-2 w-2 shrink-0 rounded-full " +
            (agent.running ? "bg-green-500" : "bg-muted-foreground/40")
          }
          title={agent.running ? "running" : "not running"}
        />
        <span className="font-medium">{agent.name}</span>
        <span className="truncate text-xs text-muted-foreground">
          {agent.role}
        </span>
        <button
          type="button"
          onClick={onOpen}
          className="ml-auto rounded border border-primary/45 px-2 py-0.5 text-xs font-medium hover:bg-primary/10"
        >
          {agent.running ? "Show" : "Open"}
        </button>
      </div>
      <div className="mt-1.5 flex items-center gap-1.5">
        <input
          value={task}
          onChange={(e) => onTask(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") onDispatch();
          }}
          placeholder="dispatch a task…"
          className="min-w-0 flex-1 rounded border border-border/60 bg-transparent px-2 py-1 text-xs outline-none focus:border-primary/50"
        />
        <button
          type="button"
          disabled={!task.trim()}
          onClick={onDispatch}
          className="rounded border border-border/60 px-2 py-1 text-xs hover:bg-foreground/5 disabled:opacity-40"
        >
          Send
        </button>
      </div>
    </li>
  );
}

function AgentSelect({
  value,
  onChange,
  placeholder,
  agents,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder: string;
  agents: OrchestrationAgent[];
}) {
  return (
    <select
      value={value}
      onChange={(e) => onChange(e.target.value)}
      className="rounded border border-border/60 bg-background px-1.5 py-1 text-xs outline-none focus:border-primary/50"
    >
      <option value="">{placeholder}</option>
      {agents.map((a) => (
        <option key={a.name} value={a.name}>
          {a.name}
        </option>
      ))}
    </select>
  );
}
