import { AgentStatus } from "@/api/generated/model";

// The agent of a swimlane, said once for every screen of the app. The contract's `AgentStatus` is connected, disconnected or
// never seen; the design's dot is online, degraded or offline. The contract has no "degraded", so the ring is never drawn
// (design/DEVIATIONS.md). A state the client does not know is offline: a dot must never say "online" on a guess.

export type AgentView = {
  /** The dot to draw. */
  readonly state: "online" | "offline";
  /** The words next to it. */
  readonly label: string;
  readonly connected: boolean;
};

export function agentView(status: AgentStatus): AgentView {
  switch (status) {
    case AgentStatus.connected:
      return { state: "online", label: "Online", connected: true };
    case AgentStatus.disconnected:
      return { state: "offline", label: "Offline", connected: false };
    case AgentStatus.never_seen:
      return { state: "offline", label: "Never seen", connected: false };
    default:
      return { state: "offline", label: "Offline", connected: false };
  }
}

/**
 * When a disconnected agent was last heard from. The contract has no "last seen" for an agent; the time of its last scan is the
 * last time it reported anything, so that is what the screens show ("Offline · 14 min ago") and say so in the title.
 */
export function offlineSince(status: AgentStatus, lastScanAt: number | null): number | null {
  return status === AgentStatus.disconnected ? lastScanAt : null;
}
