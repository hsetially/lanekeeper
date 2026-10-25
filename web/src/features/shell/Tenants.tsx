import { SafeText } from "@/components/app/SafeText";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { GitBranch } from "@/icons";
import { cn } from "@/lib/utils";

// A swimlane has a SET of tenants (D84), not one branch. Wherever the design shows "the branch", the app shows the first
// tenant and a "+N" chip, and the whole set in a tooltip. An unadopted swimlane may have none: that is shown, not hidden.

/** `sit7` and `+2` for ["sit7", "sit8", "sit9"]; `none` for an empty set. */
export function tenantSummary(tenants: readonly string[]): { first: string | null; more: number; all: string } {
  return { first: tenants[0] ?? null, more: Math.max(0, tenants.length - 1), all: tenants.join(", ") };
}

export function TenantSet({ tenants, className }: { tenants: readonly string[]; className?: string }) {
  const { first, more, all } = tenantSummary(tenants);
  return (
    <span data-slot="tenant-set" className={cn("inline-flex max-w-full min-w-0 items-center gap-1 font-mono text-xs text-muted-foreground", className)}>
      <GitBranch aria-hidden="true" className="size-3 shrink-0" />
      {first === null ? <span className="font-sans">No tenants</span> : <SafeText text={first} className="min-w-0 truncate" />}
      {more > 0 ? (
        <Tooltip>
          <TooltipTrigger asChild>
            <span tabIndex={-1} className="inline-flex h-4 items-center rounded-sm border border-border px-1 text-[11px] leading-none">
              +{more}
              <span className="sr-only"> more tenants: {all}</span>
            </span>
          </TooltipTrigger>
          <TooltipContent>
            <SafeText text={all} className="font-mono" />
          </TooltipContent>
        </Tooltip>
      ) : null}
    </span>
  );
}
