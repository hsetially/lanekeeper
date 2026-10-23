import { Severity } from "@/api/generated/model";
import { badgeBase } from "@/components/ui/badge";
import { CircleX, Info, ShieldAlert, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";
import { SEVERITY_LABEL } from "./severity";

// THE severity badge. A severity as an icon and a word, never a colour alone, in the colours of the design's Findings screen
// (`.sv-*`): High is the conflict red, Medium the warning amber, Low the info blue with no fill. The design has three levels and
// the contract four, so Critical takes High's colour and is told from it by its shield, its word and a heavier weight
// (design/DEVIATIONS.md, "Findings: severity"). Low has no fill, so the levels differ in greyscale as well.
//
// It replaces the three badges the screens built for themselves (the swimlane Findings tab, the Findings screen and the file
// history), which drew High and Low in different colours from each other.

const LOOK = {
  [Severity.critical]: { Icon: ShieldAlert, tone: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg font-semibold" },
  [Severity.high]: { Icon: CircleX, tone: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg" },
  [Severity.medium]: { Icon: TriangleAlert, tone: "border-warning-border bg-warning-bg text-warning-fg" },
  [Severity.low]: { Icon: Info, tone: "border-info-border bg-transparent text-info-fg" },
} as const;

export type SeverityBadgeProps = {
  /** A row nobody rated has none: the badge draws nothing for it, and never makes a level up. */
  severity: Severity | null | undefined;
  /** `High severity`, as the header of a drawer reads. */
  long?: boolean;
  className?: string;
};

export function SeverityBadge({ severity, long = false, className }: SeverityBadgeProps) {
  if (severity === null || severity === undefined) return null;
  const { Icon, tone } = LOOK[severity];
  return (
    <span data-slot="severity-badge" data-severity={severity} className={cn(badgeBase, tone, className)}>
      <Icon aria-hidden="true" />
      {SEVERITY_LABEL[severity]}
      {long ? " severity" : ""}
    </span>
  );
}
