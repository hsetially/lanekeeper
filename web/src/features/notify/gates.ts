import type { AgentStatus } from "@/api/generated/model";
import { useWriteGate, type GateResult } from "@/components/app/write-gate";
import { agentView } from "@/features/swimlanes/agent";

// Who may press a button that acts on a swimlane's services: an Operator, while live updates are up (the shell's gate), and
// while the swimlane's agent is connected, because a notification and a restart both go through it. The first reason that
// applies is the one shown: the permission before everything, because waiting does not fix it. The server enforces all of it.

/** `base` is `useWriteGate("operator")`. */
export function serviceWriteGate(base: GateResult, agent: AgentStatus): GateResult {
  if (!base.allowed) return base;
  return agentView(agent).connected ? base : { allowed: false, reason: "Unavailable while the agent is offline" };
}

export function useServiceWriteGate(agent: AgentStatus): GateResult {
  return serviceWriteGate(useWriteGate("operator"), agent);
}
