import type { ReactElement } from "react";
import { pickupSummary } from "@/features/swimlanes/pickup";
import { cn } from "@/lib/utils";
import { countsToShow, type PickupCountPairs } from "./model";
import { PickupChip } from "./PickupChip";

// How many services are in each pickup state that is not live: the Pickup column of the swimlane list and the chips under a
// swimlane's name. There is one chip for each state, with the count of THAT state. (The first version of these screens drew one
// chip in the colour of the most urgent state with the total of the urgent ones beside it, so a swimlane with one service that
// needs a config-server restart and two that need a notify read "3 Needs config-server restart".)
//
// The counts are `SwimlaneSummary.pickup_counts`: pairs of a state and a count, only the states that are not live, absent when
// nothing is waiting. Nothing is computed here but the order and the sums of a state that is listed twice.

export type PickupCountsProps = {
  /** `SwimlaneSummary.pickup_counts` (or the detail's). */
  counts: PickupCountPairs;
  /** `compact`: an icon and a count in each chip, for a table cell. `full`: the words too, for a header. */
  density?: "full" | "compact";
  /** `actionable` (the default): the two states in which a person has to notify or restart. `all` adds live-within-cache. */
  include?: "actionable" | "all";
  /** What to draw when there is nothing to show. `zero`: a muted 0 with the words, for a column that must not be empty. */
  whenNone?: "nothing" | "zero";
  className?: string | undefined;
};

export function PickupCounts({ counts, density = "full", include = "actionable", whenNone = "nothing", className }: PickupCountsProps): ReactElement | null {
  const shown = countsToShow(counts, include);
  if (shown.length === 0) {
    if (whenNone === "nothing") return null;
    // The words are the summary's (what is left over, when something is), so the 0 never says more than the hub did.
    return (
      <span data-slot="pickup-counts" data-empty="true" title={pickupSummary(counts).title} className={cn("tabular-nums text-muted-foreground", className)}>
        0
      </span>
    );
  }
  return (
    <span data-slot="pickup-counts" className={cn("inline-flex min-w-0 items-center", density === "compact" ? "flex-nowrap gap-1" : "flex-wrap gap-x-2 gap-y-1.5", className)}>
      {shown.map(([state, count]) => (
        <PickupChip key={state} state={state} subject="count" count={count} density={density} />
      ))}
    </span>
  );
}
