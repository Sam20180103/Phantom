import { invoke, Channel } from "@tauri-apps/api/core";
import { currentWorkspaceEnv } from "@/modules/workspace";

/**
 * The multi-CLI orchestration primitives, browser side.
 *
 * These wrap the four `agent_*` server commands. Content between agents travels
 * by file (`.phantom/handoff/*.md`); these calls only name a session and poke
 * it — see the design in `Phantom-任务清单.md` §二 P1.
 */

/** One agent from `.phantom/agents.yaml`, with live-session state folded in. */
export type OrchestrationAgent = {
  name: string;
  cmd: string;
  role: string;
  running: boolean;
};

/** The `phantom-agent-<name>` session name the terminal attaches to. */
export function agentSessionName(name: string): string {
  return `phantom-agent-${name}`;
}

/** Reads `.phantom/agents.yaml` and marks which agents have a live session. */
export async function listAgents(cwd: string): Promise<OrchestrationAgent[]> {
  try {
    return await invoke<OrchestrationAgent[]>("agent_list", {
      cwd,
      workspace: currentWorkspaceEnv(),
    });
  } catch {
    // No config, or a backend without rmux — nothing to orchestrate.
    return [];
  }
}

/** Create-or-reuse an agent's session (no terminal); caller then attaches. */
export async function ensureAgent(
  name: string,
  cols: number,
  rows: number,
  cwd: string,
): Promise<string> {
  const r = await invoke<{ session: string }>("agent_ensure", {
    name,
    cols,
    rows,
    cwd,
  });
  return r.session;
}

/** Send a task line to an agent's session (types it + Enter). */
export async function dispatchAgent(name: string, task: string): Promise<void> {
  await invoke("agent_dispatch", { name, task });
}

/**
 * Send a message to an agent, but wait until it looks idle first (so it lands
 * at a prompt, not mid-command). `timeoutMs` bounds the wait; the server sends
 * anyway once it elapses. Resolves to how it was sent: `"idle"` or `"timeout"`.
 */
export async function nudgeAgent(
  name: string,
  msg: string,
  timeoutMs?: number,
): Promise<"idle" | "timeout"> {
  const r = await invoke<{ sent: "idle" | "timeout" }>("agent_nudge", {
    name,
    msg,
    ...(timeoutMs != null ? { timeoutMs } : {}),
  });
  return r.sent;
}

/** One step the orchestrator brain took, streamed live as it runs. */
export type OrchestrationStep =
  | { kind: "dispatch"; agent: string; task: string }
  | { kind: "nudge"; agent: string; message: string }
  | { kind: "read"; file: string }
  | { kind: "list" }
  | { kind: "finish"; summary: string }
  | { kind: "note"; text: string }
  | { kind: "stopped" };

/**
 * The brain that drives the orchestration, taken from a Models-panel provider so
 * it's configured once. The key never crosses the wire — the server reads it
 * from its own secrets by `keyService`/`keyAccount`.
 */
export type OrchestratorBrain = {
  /** Full chat-completions URL (OpenAI-compatible). */
  endpoint: string;
  model: string;
  keyService: string;
  keyAccount: string;
};

/**
 * Runs the orchestrator toward a goal, streaming each step to `onStep`.
 *
 * `brain` picks the LLM (a Models-panel provider); omit it to fall back to the
 * `orchestrator:` block in `.phantom/agents.yaml`. The brain decides which
 * agents to dispatch/nudge and reads their handoff files; this call resolves
 * when it finishes, hits its step cap, or is stopped. Same primitives as the
 * manual buttons — only "who presses" differs.
 */
export async function orchestrate(
  goal: string,
  cwd: string,
  onStep: (step: OrchestrationStep) => void,
  brain?: OrchestratorBrain,
): Promise<{ status: string }> {
  const onEvent = new Channel<OrchestrationStep>();
  onEvent.onmessage = onStep;
  return await invoke<{ status: string }>("orchestrate", {
    goal,
    cwd,
    onEvent,
    ...(brain && {
      endpoint: brain.endpoint,
      model: brain.model,
      keyService: brain.keyService,
      keyAccount: brain.keyAccount,
    }),
  });
}

/** Signals a running orchestration to stop before its next step. */
export async function stopOrchestration(): Promise<void> {
  await invoke("orchestrate_stop", {});
}
