import { Severity } from "@/api/generated/model";
import { badgeBase } from "@/components/ui/badge";
import { ArrowDown, Info, ShieldAlert, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";

// A severity as a label and an icon (never colour alone), in the design's colours: critical in the conflict family, high in
// danger, medium in warning, low in unknown. The attribution and severity work of task T5 may replace this with its own.

const SPEC = {
  [Severity.critical]: { label: "Critical", Icon: ShieldAlert, tone: "border-status-conflict-border bg-status-conflict-bg text-status-conflict-fg" },
  [Severity.high]: { label: "High", Icon: TriangleAlert, tone: "border-danger-border bg-danger-bg text-danger-fg" },
  [Severity.medium]: { label: "Medium", Icon: Info, tone: "border-warning-border bg-warning-bg text-warning-fg" },
  [Severity.low]: { label: "Low", Icon: ArrowDown, tone: "border-status-unknown-border bg-status-unknown-bg text-status-unknown-fg" },
} as const;

export function SeverityBadge({ severity, className }: { severity: Severity; className?: string }) {
  const spec = SPEC[severity];
  return (
    <span data-slot="severity-badge" data-severity={severity} className={cn(badgeBase, spec.tone, className)}>
      <spec.Icon aria-hidden="true" />
      {spec.label}
    </span>
  );
}
