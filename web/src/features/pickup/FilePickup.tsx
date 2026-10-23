import type { ReactElement } from "react";
import type { PickupState } from "@/api/generated/model";
import { PickupChip } from "@/components/pickup";

// The pickup chip in the head of the file view: when the services that read this file get its new version. It takes the file's
// entry (`FileEntry.pickup_state`) and shows any state the hub gave, Live included; the hub does not fill the field in yet, so a
// file without one draws nothing (no state is not "Live"). The screen puts it at the end of the title row (design/
// DEVIATIONS.md: "File: header, pickup"): the row of the file's facts has no room for the words at 1440 wide. `density="compact"` is
// an icon alone, for a place that is tighter still.

export function FilePickup({ entry, density = "full" }: { entry: { readonly pickup_state: PickupState | null } | null | undefined; density?: "full" | "compact" }): ReactElement | null {
  if (entry === null || entry === undefined) return null;
  return <PickupChip state={entry.pickup_state} subject="file" density={density} />;
}
