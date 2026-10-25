import type { Attribution } from "@/api/generated/model";
import { cn } from "@/lib/utils";
import { ConfidenceMeter } from "./ConfidenceMeter";
import { describeAttribution } from "./model";

// The Attribution column of an audit table: how Lanekeeper identified the author (Via Lanekeeper, Edited on the NFS VM, Sync job,
// NFS client, File owner hint, Unknown) over a quarter-step track and the level in words. Plain text, not a control, because the
// row it sits in opens the event. The source `unknown` has no score, so it has no track (a Low next to Unknown would be a score
// that does not exist); the evidence behind it is in the event's drawer, as `AttributionEvidence`.

export function AttributionSourceCell({ attribution, className }: { attribution: Attribution | null; className?: string }) {
  if (attribution === null) {
    return (
      <span data-slot="attribution-source" className={cn("text-muted-foreground", className)}>
        –
      </span>
    );
  }
  const view = describeAttribution(attribution);
  return (
    <span data-slot="attribution-source" data-source={view.source} className={cn("flex min-w-0 flex-col items-start gap-0.5", className)}>
      <span className={cn("max-w-full truncate text-xs leading-4", !view.known && "text-muted-foreground")}>{view.sourceLabel}</span>
      {view.known ? <ConfidenceMeter variant="track" confidence={view.confidence.level} /> : null}
    </span>
  );
}
