import { cn } from "@/lib/utils";

type AgentState = "online" | "degraded" | "offline";

const SHAPES = {
  online: { shape: "filled", label: "Agent online", classes: "rounded-full bg-agent-online" },
  degraded: { shape: "ring", label: "Agent degraded", classes: "rounded-full border-2 border-agent-degraded bg-transparent" },
  offline: { shape: "diamond", label: "Agent offline", classes: "rotate-45 bg-agent-offline" },
} as const;

/** The agent's connection: a filled dot, a ring or a diamond, so the three differ without colour. */
export function AgentDot({ state, className }: { state: AgentState; className?: string }) {
  const spec = SHAPES[state];
  return <span data-slot="agent-dot" data-shape={spec.shape} role="img" aria-label={spec.label} className={cn("inline-block size-2 shrink-0", spec.classes, className)} />;
}
