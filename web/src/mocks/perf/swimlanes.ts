// A list of swimlanes for the route-change pages of the P10 runs: enough rows to be a real list, valid against the contract.
import type { SwimlaneSummary, SwimlaneSummaryPage } from "../../api/generated/model/index.ts";

export const SWIMLANE_COUNT = 200;

function summary(i: number): SwimlaneSummary {
  return {
    id: `lane-${String(i).padStart(4, "0")}`,
    display_name: `Lane ${i}`,
    tenants: [`tenant-${i % 9}`],
    agent: i % 11 === 0 ? "disconnected" : "connected",
    last_scan_at: 1_760_000_000_000 + i * 60_000,
    base_version: null,
    drift_counts: [["in_sync", 400 + i]],
    finding_count: i % 5,
  };
}

export function swimlanesBody(): SwimlaneSummaryPage {
  return { items: Array.from({ length: SWIMLANE_COUNT }, (_, i) => summary(i + 1)), next: null };
}
