// The design-parity scenarios of slice S6a (the Findings screen): the data of the Findings board of the design
// (`Lanekeeper Findings and Adoption.dc.html`, page=findings, which imports `Lanekeeper Findings Page.dc.html`), so that the app
// can be compared with the reference frames. The signed-in person and the sidebar are the Shell's (the Admin Priya Shah, six
// swimlanes, 6 pending approvals); the swimlanes' finding counts are the ones the eleven findings add up to (11, as the sidebar
// of the design says), and every agent is online except in the offline scenario.
//
// What the board's filter frames need is not a scenario of its own: the answer to `GET /api/v1/findings` filters by the
// parameters it gets, as the hub does (./server.ts), so `?lanes=sit&code=C8` is the "no match" frame.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { Finding, FindingKind, Severity, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { scenarios as shell } from "../shell/index.ts";
import { DESIGN_FINDINGS, KIND_EXAMPLES, NOW, finding, laneCounts } from "./fixtures.ts";
import { cursorAfter, findingPage } from "./server.ts";

const base = (id: string): Scenario => {
  const found = shell.find((s) => s.id === id);
  if (found === undefined) throw new Error(`the shell scenario ${id} is missing`);
  return found;
};

/** The answers of one hub: the shell's (the session, the logout, the search the palette makes, the approvals) and the two lists. */
function scenario(id: string, description: string, from: Scenario, lanes: readonly SwimlaneSummary[], findings: readonly Finding[], pages: readonly Partial<MockContext>[] = []): Scenario {
  const list: MockResponse = {
    method: "GET",
    path: "/api/v1/findings",
    body: (context: MockContext) => findingPage(findings, context.query),
    sample: { query: new URLSearchParams() },
    ...(pages.length === 0 ? {} : { samples: pages }),
  };
  return {
    id,
    description,
    responses: [...from.responses.filter((r) => r.path !== "/api/v1/swimlanes"), { method: "GET", path: "/api/v1/swimlanes", body: { items: lanes, next: null } }, list],
  };
}

/** A thousand and fifty findings, spread over the six swimlanes and the twelve checks: more than the screen loads. */
function crowd(count: number): Finding[] {
  const lanes = ["sit", "sitb", "sitc", "presita", "presitb", "presitc"] as const;
  const kinds = Object.keys(KIND_EXAMPLES) as FindingKind[];
  const severities: Severity[] = ["low", "medium", "high", "critical"];
  return Array.from({ length: count }, (_, i) =>
    finding(i + 1, {
      kind: kinds[i % kinds.length] ?? "drift",
      swimlane: lanes[i % lanes.length] ?? "sit",
      severity: severities[i % severities.length] ?? "low",
      path: `tx-infinity-api/file-${i % 97}.yml`,
      message: `Finding ${i + 1}`,
    }),
  );
}

const TRUNCATED = crowd(1_050);

export const scenarios: readonly Scenario[] = [
  scenario("design-findings-open", "Findings frames 1a to 1t: eleven open findings across five swimlanes, sorted by severity, an Admin.", base("design-shell-admin"), laneCounts(DESIGN_FINDINGS), DESIGN_FINDINGS),
  scenario("design-findings-viewer", "Findings frame 1s: the same findings for a Viewer, who has no action on them.", base("design-shell-viewer"), laneCounts(DESIGN_FINDINGS), DESIGN_FINDINGS),
  scenario("design-findings-empty", "Findings frame 1g and State E07: nothing is open on any swimlane.", base("design-shell-admin"), laneCounts([]), []),
  scenario("design-findings-all-kinds", "One finding of each of the twelve checks, C1 to C12, a critical one among them.", base("design-shell-admin"), laneCounts(Object.values(KIND_EXAMPLES)), Object.values(KIND_EXAMPLES)),
  scenario(
    "design-findings-offline",
    "State O06: the agent of presitb is offline, so its findings are as of its last scan.",
    base("design-shell-admin"),
    laneCounts(DESIGN_FINDINGS, { presitb: { agent: "disconnected", last_scan_at: NOW - 14 * 60_000 } }),
    DESIGN_FINDINGS,
  ),
  scenario(
    "design-findings-truncated",
    "State T03: more findings than the screen loads (1,050), so the list is cut off and says so.",
    base("design-shell-admin"),
    laneCounts(TRUNCATED),
    TRUNCATED,
    [{ query: new URLSearchParams() }, { query: new URLSearchParams({ cursor: cursorAfter(500) }) }, { query: new URLSearchParams({ cursor: cursorAfter(100), limit: "100" }) }],
  ),
];
