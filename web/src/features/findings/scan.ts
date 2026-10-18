import type { SwimlaneSummary } from "@/api/generated/model";

/** When the most recent scan of any of these swimlanes ran: null when none has been scanned yet (the contract allows a swimlane that was never scanned). */
export function latestScan(lanes: readonly Pick<SwimlaneSummary, "last_scan_at">[]): number | null {
  let latest: number | null = null;
  for (const lane of lanes) {
    if (lane.last_scan_at !== null && (latest === null || lane.last_scan_at > latest)) latest = lane.last_scan_at;
  }
  return latest;
}
