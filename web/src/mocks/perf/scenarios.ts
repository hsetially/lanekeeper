// The scenarios of the P10 runs: a signed-in Editor (so the editor warm-up is on), the 5,000-row grid, the 6,000-line diff
// with an ETag, and a list of swimlanes for the route-change pages. `perf-full` has the worst-case diff, every line changed,
// and `perf-viewer` is the same data for a Viewer, who does not get the editor warmed.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.

import type { Role } from "../../api/generated/model/index.ts";
import { designScenarios } from "../design/index.ts";
import type { MockContext, MockResponse, Scenario } from "../scenario.ts";
import { meBody } from "../me.ts";
import { makeComparison, type DiffShape } from "./diff.ts";
import { gridBody } from "./grid.ts";
import { swimlanesBody } from "./swimlanes.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

/** The two shapes: the realistic diff, and the worst case. The ETag names the shape, as the hub's names the two hashes. */
const REALISTIC = { shape: { lines: 6000, changeEvery: 10 } as DiffShape, etag: '"perf-diff-6000-realistic-1"' };
const FULL = { shape: { lines: 6000, full: true } as DiffShape, etag: '"perf-diff-6000-full-1"' };

function scenario(id: string, description: string, role: Role, { shape, etag }: { shape: DiffShape; etag: string }): Scenario {
  // Made once: the body is the same for every request, and 12,000 lines are not worth making each time.
  let comparison: ReturnType<typeof makeComparison> | undefined;
  return {
    id,
    description,
    responses: [
      { method: "GET", path: "/api/v1/me", body: meBody(role), headers: { "set-cookie": csrfCookie } },
      { method: "POST", path: "/auth/logout", status: 204 },
      {
        method: "GET",
        path: "/api/v1/grid",
        body: (ctx: MockContext) => gridBody(ctx.query),
        sample: { query: new URLSearchParams("limit=500") },
        samples: [{ query: new URLSearchParams("limit=500&cursor=o2500") }, { query: new URLSearchParams("limit=500&cursor=o4500") }, { query: new URLSearchParams("limit=1") }],
      },
      { method: "GET", path: "/api/v1/compare", headers: { etag }, body: () => (comparison ??= makeComparison(shape)) },
      { method: "GET", path: "/api/v1/swimlanes", body: swimlanesBody() },
    ],
  };
}

/**
 * The answers of several design scenarios in one, for a run that walks across screens: each part gives the responses its filter
 * keeps, and the first part to answer a method and path is the one that does (so the order says whose Swimlanes, whose proposals).
 * Nothing is written here that a part does not already say, and each part has been checked against the contract.
 */
function composed(id: string, description: string, parts: ReadonlyArray<{ from: string; only?: (response: MockResponse) => boolean }>): Scenario {
  const seen = new Set<string>();
  const responses: MockResponse[] = [];
  for (const { from, only = () => true } of parts) {
    const part = designScenarios[from];
    if (part === undefined) throw new Error(`perf scenario ${id}: there is no design scenario ${from}`);
    for (const response of part.responses.filter(only)) {
      const key = `${response.method} ${response.path}`;
      if (seen.has(key)) continue;
      seen.add(key);
      responses.push(response);
    }
  }
  return { id, description, responses };
}

const under = (...prefixes: string[]) => (response: MockResponse) => prefixes.some((prefix) => response.path === prefix || response.path.startsWith(`${prefix}/`));

export const perfScenarios: Record<string, Scenario> = {
  "perf-routes": composed(
    "perf-routes",
    "An Admin walking the screens of the sidebar with their data cached: forty swimlanes, the approval queue, the findings, the comparison grid.",
    [
      { from: "design-approvals-pending", only: under("/api/v1/proposals") },
      { from: "design-findings-open", only: under("/api/v1/findings") },
      { from: "design-compare-default", only: under("/api/v1/grid", "/api/v1/compare", "/api/v1/git/refs") },
      { from: "design-swimlanes-forty" },
      { from: "design-shell-admin" },
    ],
  ),
  perf: scenario("perf", "An Editor with the P10 data: a 5,000-row grid of 40 swimlanes, a 6,000-line diff, 200 swimlanes.", "editor", REALISTIC),
  "perf-full": scenario("perf-full", "As `perf`, with the worst-case diff: every line of the 6,000 changed.", "editor", FULL),
  "perf-viewer": scenario("perf-viewer", "As `perf`, for a Viewer: the editor is not warmed.", "viewer", REALISTIC),
};
