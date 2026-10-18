import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

/** SIT is outlined and PRESIT is filled muted, so a lane's tier reads without colour. */
export function TierBadge({ tier }: { tier: string }) {
  return <Badge variant={tier.toLowerCase() === "presit" ? "muted" : "outline"}>{tier.toUpperCase()}</Badge>;
}

/**
 * The tier as the Shell draws it in the swimlane switcher and the swimlane list: mono, 12/500, spaced. SIT is an outline; PRESIT
 * is filled the other way round (muted-foreground behind the page's colour), so the two differ in greyscale as well as in words.
 */
export function TierChip({ tier, className }: { tier: string; className?: string }) {
  const pre = tier.toLowerCase() === "presit";
  return (
    <span
      data-slot="tier-chip"
      data-tier={pre ? "presit" : "sit"}
      className={cn(
        "inline-flex h-5 shrink-0 items-center rounded-sm border px-1.5 font-mono text-xs font-medium tracking-[0.04em]",
        pre ? "border-muted-foreground bg-muted-foreground text-background" : "border-input bg-transparent text-foreground",
        className,
      )}
    >
      {tier.toUpperCase()}
    </span>
  );
}
