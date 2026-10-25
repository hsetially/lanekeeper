// The design-parity scenarios of slice S4 (the Compare page): the grid across swimlanes and the whole-swimlane compare of the
// design (`Lanekeeper Compare Page.dc.html`, shown by the Shell with page=compare). One scenario serves both modes: the grid, the
// comparison of two swimlanes (the drawer), the tree of a whole-swimlane comparison and its files, and the Git refs of the side
// pickers; the address of the frame (file, lanes, only, left, right) chooses what is on screen. The others change one thing.
//
//   design-compare-default    six swimlanes, all agents online (frames 1a to 1f, 2a to 2e)
//   design-compare-offline    presitb's agent is offline, its files as of 14 minutes ago (1i, 1j, 2i)
//   design-compare-toomany    43 swimlanes in the list, for the address that names all of them (1h)
//   design-compare-empty      the grid has no row and the tree no file (E04, 1k with a search)
//   design-compare-identical  the two sides are identical (2f, 2g)
//   design-compare-error      every comparison answers 500 with a request id (R04)
//   design-compare-large      FORTY swimlanes and 5,000 rows, served in the pages the real screen asks for (P10)
//
// The loading frames (1g, 2h, L04) are not scenarios: the harness holds the answers.
//
// `design-compare-large` is what the Phase C performance specs point at: open
//   /compare?file=services%2Fpayments%2Fapplication.yml&lanes=swimlane-01,...,swimlane-40&only=all
// (the 40 ids are GRID_LANES in src/mocks/perf/grid.ts). The real screen asks for 50 rows, and 500 rows for each page after that,
// as the person scrolls to within a screen of the end of the rows it has, or presses "Load 500 more". Eleven requests load all
// 5,000 rows; `End` on the focused grid, repeated, does it from the keyboard.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { Me, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { GRID_LANES, gridBody } from "../../perf/grid.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { PRIYA, SHELL_LANES, scenarios as shellScenarios } from "../shell/index.ts";
import { CORE_FILE, DESIGN_LANES, designCompare, designGrid, designRefs, designTree } from "./data.ts";

const NOW = 1_760_000_000_000;
const MIN = 60_000;
const SIX = DESIGN_LANES.map((lane) => lane.n).join(",");
const csrf = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

const online: readonly SwimlaneSummary[] = SHELL_LANES.map((lane) => ({ ...lane, agent: "connected" }));
const withPresitbOffline: readonly SwimlaneSummary[] = SHELL_LANES.map((lane) => (lane.id === "presitb" ? { ...lane, agent: "disconnected" as const, last_scan_at: NOW - 14 * MIN } : lane));

/** The 43 names the design's "too many" frame lists: sit, sitb to sitz, presita to presitq. */
const MANY: readonly SwimlaneSummary[] = [
  "sit",
  ...Array.from({ length: 25 }, (_, i) => `sit${String.fromCharCode(98 + i)}`),
  ...Array.from({ length: 17 }, (_, i) => `presit${String.fromCharCode(97 + i)}`),
].map((id, i): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`sit${i + 1}`],
  agent: "connected",
  last_scan_at: NOW,
  base_version: { tag: "v2.3.1", distance: 0, commit: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c" },
  drift_counts: [],
  finding_count: 0,
  tier: id.startsWith("presit") ? "PRESIT" : "SIT",
  baseline: { state: "confirmed", by: null, at: null },
}));

const LARGE: readonly SwimlaneSummary[] = GRID_LANES.map((id, i): SwimlaneSummary => ({
  id,
  display_name: id,
  tenants: [`tenant-${String(i + 1).padStart(2, "0")}`],
  agent: "connected",
  last_scan_at: NOW,
  base_version: { tag: "v2.3.1", distance: 0, commit: "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c" },
  drift_counts: [],
  finding_count: 0,
  tier: i % 2 === 0 ? "SIT" : "PRESIT",
  baseline: { state: "confirmed", by: null, at: null },
}));

const q = (text: string): URLSearchParams => new URLSearchParams(text);
const grid = (extra: string) => ({ query: q(`file=${encodeURIComponent(CORE_FILE)}&swimlanes=${SIX}${extra}`) });
const cmp = (left: string, right: string, file: string) => ({ query: q(`left=${encodeURIComponent(left)}&right=${encodeURIComponent(right)}&file=${encodeURIComponent(file)}`) });

type Options = {
  id: string;
  description: string;
  lanes: readonly SwimlaneSummary[];
  me?: Me;
  grid?: "design" | "empty" | "error" | "large";
  tree?: "design" | "empty" | "identical" | "error";
};

function scenario(o: Options): Scenario {
  const me = o.me ?? meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" });
  const shell = shellScenarios.find((s) => s.id === "design-shell-admin")?.responses.filter((r) => r.path !== "/api/v1/swimlanes" && r.path !== "/api/v1/me") ?? [];
  const fail = (): MockResponse["body"] => problemBody(500, "internal", "Internal error");
  const gridMode = o.grid ?? "design";
  const treeMode = o.tree ?? "design";
  const lanes = o.lanes.map((l) => l.id);
  const gridResponse: MockResponse =
    gridMode === "error"
      ? { method: "GET", path: "/api/v1/grid", status: 500, body: fail() }
      : {
          method: "GET",
          path: "/api/v1/grid",
          body:
            gridMode === "large"
              ? (ctx: MockContext) => gridBody(ctx.query)
              : gridMode === "empty"
                ? (ctx: MockContext) => ({ swimlanes: (ctx.query.get("swimlanes") ?? "").split(",").filter((s) => s !== ""), rows: { items: [], next: null } })
                : (ctx: MockContext) => designGrid(ctx.query),
          sample: gridMode === "large" ? { query: q("limit=50") } : grid("&only_differing=true&limit=50"),
          samples:
            gridMode === "large"
              ? [{ query: q("limit=500&cursor=o50") }, { query: q("limit=500&cursor=o4550") }]
              : [grid("&only_differing=true&limit=500&cursor=o50"), grid("&limit=50"), grid("&channel=atm&limit=500&cursor=o1000"), { query: q(`file=${encodeURIComponent(CORE_FILE)}&swimlanes=${lanes.slice(0, 3).join(",")}&limit=50`) }],
        };
  const compareResponse: MockResponse =
    gridMode === "error" || treeMode === "error"
      ? { method: "GET", path: "/api/v1/compare", status: 500, body: fail() }
      : {
          method: "GET",
          path: "/api/v1/compare",
          body: (ctx: MockContext) => (treeMode === "identical" ? { ...designCompare(ctx.query), identical: true, hunks: [], setting_changes: [] } : designCompare(ctx.query)),
          sample: cmp("effective:sit", "effective:sitb", CORE_FILE),
          samples: [
            cmp("effective:sitb", "effective:sitc", CORE_FILE),
            cmp("effective:sitb", "effective:sitc", "tx-infinity-api/tx-infinity-gateway.yml"),
            cmp("nfs:sitb", "git:tenant@sit7", CORE_FILE),
            cmp("effective:sitb", "effective:sitc", "resources/receipt-logo.jpeg"),
            cmp("effective:sitb", "git:base@v2.3.1", "resources/receipt-footer.txt"),
            cmp("effective:sitb", "effective:sitc", "tx-infinity-api/tx-infinity-limits.yml"),
          ],
          // The hub tags a comparison, and the app asks again with the tag (a 304 keeps the answer it has).
          headers: { etag: '"design-compare-1"' },
        };
  const treeResponse: MockResponse =
    treeMode === "error" || gridMode === "error"
      ? { method: "GET", path: "/api/v1/compare/tree", status: 500, body: fail() }
      : {
          method: "GET",
          path: "/api/v1/compare/tree",
          body: (ctx: MockContext) => (treeMode === "empty" || gridMode === "large" ? { entries: { items: [], next: null }, skipped_identical_subtrees: 0 } : designTree(ctx.query, treeMode === "identical")),
          sample: { query: q("left=effective%3Asitb&right=effective%3Asitc&limit=500") },
          samples: [{ query: q("left=nfs%3Asitb&right=git%3Atenant%40sit7&limit=500") }, { query: q("left=nfs%3Asitb&right=git%3Abase%40v2.3.1&limit=500") }],
        };
  return {
    id: o.id,
    description: o.description,
    responses: [
      { method: "GET", path: "/api/v1/me", body: me, headers: { "set-cookie": csrf } },
      ...shell,
      { method: "GET", path: "/api/v1/swimlanes", body: { items: o.lanes, next: null } },
      gridResponse,
      compareResponse,
      treeResponse,
      { method: "GET", path: "/api/v1/git/refs", body: (ctx: MockContext) => designRefs(ctx.query), sample: { query: q("repo=tenant&kind=branch") }, samples: [{ query: q("repo=base&kind=tag") }, { query: q("repo=tenant&kind=branch&q=sit1") }] },
    ],
  };
}

export const scenarios: readonly Scenario[] = [
  scenario({ id: "design-compare-default", description: "The Compare page of the design: six swimlanes with their agents online, 5,000 settings of the core file in three channels, the files of a whole-swimlane comparison of sitb with sitc, sit7 or v2.3.1.", lanes: online }),
  scenario({ id: "design-compare-offline", description: "As design-compare-default, with the agent of presitb offline since 14 minutes before the frozen clock: its column and its side show the files as of its last scan.", lanes: withPresitbOffline }),
  scenario({ id: "design-compare-toomany", description: "As design-compare-default with 43 swimlanes in the list (sit, sitb to sitz, presita to presitq), for the address that names all of them.", lanes: MANY }),
  scenario({ id: "design-compare-empty", description: "The grid has no row (the swimlanes agree) and the whole-swimlane comparison lists no file.", lanes: online, grid: "empty", tree: "empty" }),
  scenario({ id: "design-compare-identical", description: "The two sides of a whole-swimlane comparison are identical: nothing to list, 43 directories skipped.", lanes: online, tree: "identical" }),
  scenario({ id: "design-compare-error", description: "Every comparison answers 500 with a request id.", lanes: online, grid: "error", tree: "error" }),
  scenario({
    id: "design-compare-large",
    description: "The grid of the P10 gate on the real Compare screen: forty swimlanes (swimlane-01 to swimlane-40) and 5,000 settings, served as the hub serves them, a keyset cursor and at most 500 rows a page. An Editor, so that the editor warm-up runs.",
    lanes: LARGE,
    me: meBody("editor", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }),
    grid: "large",
    tree: "empty",
  }),
];
