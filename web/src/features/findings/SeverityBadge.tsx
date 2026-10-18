import { Severity } from "@/api/generated/model";
import { badgeBase } from "@/components/ui/badge";
import { CircleX, Info, ShieldAlert, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";
import { SEVERITY_LABEL } from "./severity";

// A severity as an icon and a word, never a colour alone, in the colours of the design's Findings page: High is the conflict
// red, Medium the warning amber, Low the info blue with no fill. The design has three levels and the contract four, so Critical
// takes High's colour with the shield and a heavier weight: it is told from High by its word and its icon. The attribution and
// severity work of task T5 owns the shared severity badge and may replace this one.

const SPEC = {
  [Severity.critical]: { Icon: ShieldAlert, tone: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg font-semibold" },
  [Severity.high]: { Icon: CircleX, tone: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg" },
  [Severity.medium]: { Icon: TriangleAlert, tone: "border-warning-border bg-warning-bg text-warning-fg" },
  [Severity.low]: { Icon: Info, tone: "border-info-border bg-transparent text-info-fg" },
} as const;

/** `long` reads "High severity", as the drawer's header does. */
export function SeverityBadge({ severity, long = false, className }: { severity: Severity; long?: boolean; className?: string }) {
  const { Icon, tone } = SPEC[severity];
  return (
    <span data-slot="severity-badge" data-severity={severity} className={cn(badgeBase, tone, className)}>
      <Icon aria-hidden="true" />
      {SEVERITY_LABEL[severity]}
      {long ? " severity" : ""}
    </span>
  );
}
