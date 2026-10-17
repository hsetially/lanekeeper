// The scenario of the live-update runs (e2e/live): a signed-in Viewer and a short list of swimlanes whose first row shows how
// many times the list was asked for, in its finding count. A live event that makes the page refetch the list is then a number
// that changes on screen, which is what P13 measures (a live update reaches the screen within a second). The count is the
// scenario's own and means nothing else; the body is valid against the contract for any count.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension and `import type` for types.
import type { SwimlaneSummary, SwimlaneSummaryPage } from "../api/generated/model/index.ts";
import { meBody } from "./me.ts";
import type { Scenario } from "./scenario.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

function lane(i: number, findings: number): SwimlaneSummary {
  return {
    id: `lane-${String(i).padStart(4, "0")}`,
    display_name: `Lane ${i}`,
    tenants: [`tenant-${i}`],
    agent: "connected",
    last_scan_at: 1_760_000_000_000,
    base_version: null,
    drift_counts: [["in_sync", 400]],
    finding_count: findings,
  };
}

function liveScenario(): Scenario {
  let asked = 0;
  return {
    id: "live",
    description: "A Viewer, and a list of swimlanes whose first row counts how many times the list was asked for.",
    responses: [
      { method: "GET", path: "/api/v1/me", body: meBody("viewer"), headers: { "set-cookie": csrfCookie } },
      { method: "POST", path: "/auth/logout", status: 204 },
      {
        method: "GET",
        path: "/api/v1/swimlanes",
        body: (): SwimlaneSummaryPage => {
          asked += 1;
          return { items: [lane(1, asked), lane(2, 0), lane(3, 0)], next: null };
        },
      },
    ],
  };
}

export const liveScenarios: Record<string, Scenario> = { live: liveScenario() };
