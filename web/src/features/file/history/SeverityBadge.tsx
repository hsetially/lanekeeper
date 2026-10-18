import type { Severity } from "@/api/generated/model";
import { ArrowDown, Info, TriangleAlert } from "@/icons";
import { cn } from "@/lib/utils";

// The severity of a change, from the server's severity rules (D76): the word and an icon, never the colour alone.

const SEVERITY: Record<Severity, { label: string; Icon: typeof Info; classes: string }> = {
  low: { label: "Low", Icon: ArrowDown, classes: "border-status-unknown-border bg-status-unknown-bg text-status-unknown-fg" },
  medium: { label: "Medium", Icon: Info, classes: "border-warning-border bg-warning-bg text-warning-fg" },
  high: { label: "High", Icon: TriangleAlert, classes: "border-danger-border bg-danger-bg text-danger-fg" },
  critical: { label: "Critical", Icon: TriangleAlert, classes: "border-danger-border bg-danger-bg font-semibold text-danger-fg" },
};

export function SeverityBadge({ severity }: { severity: Severity }) {
  const { label, Icon, classes } = SEVERITY[severity];
  return (
    <span className={cn("inline-flex h-5 items-center gap-1 rounded-sm border px-1.5 text-xs leading-4 font-medium", classes)}>
      <Icon aria-hidden="true" className="size-3" />
      {label}
    </span>
  );
}
