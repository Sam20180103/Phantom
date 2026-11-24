import { useCallback, useEffect, useState } from "react";
import { listAgents, type OrchestrationAgent } from "./orchestration";

/**
 * Polls the agent roster for the given workspace root.
 *
 * Slow poll, like the detached-session banner: running-state changes when a CLI
 * is opened or exits, neither of which this connection is pushed. `cwd` locates
 * `.phantom/agents.yaml`; an empty roster (no config) renders as nothing.
 */
export function useOrchestrationAgents(cwd: string | undefined): {
  agents: OrchestrationAgent[];
  refresh: () => void;
} {
  const [agents, setAgents] = useState<OrchestrationAgent[]>([]);

  const refresh = useCallback(() => {
    if (!cwd) {
      setAgents([]);
      return;
    }
    void listAgents(cwd).then(setAgents);
  }, [cwd]);

  useEffect(() => {
    refresh();
    const timer = setInterval(refresh, 10_000);
    return () => clearInterval(timer);
  }, [refresh]);

  return { agents, refresh };
}
