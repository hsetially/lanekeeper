// The scenarios of the P10 runs: a signed-in Editor (so the editor warm-up is on), the 5,000-row grid, the 6,000-line diff
// with an ETag, and a list of swimlanes for the route-change pages. `perf-full` has the worst-case diff, every line changed,
// and `perf-viewer` is the same data for a Viewer, who does not get the editor warmed.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.

import type { Role } from "../../api/generated/model/index.ts";
import type { MockContext, Scenario } from "../scenario.ts";
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

export const perfScenarios: Record<string, Scenario> = {
  perf: scenario("perf", "An Editor with the P10 data: a 5,000-row grid of 40 swimlanes, a 6,000-line diff, 200 swimlanes.", "editor", REALISTIC),
  "perf-full": scenario("perf-full", "As `perf`, with the worst-case diff: every line of the 6,000 changed.", "editor", FULL),
  "perf-viewer": scenario("perf-viewer", "As `perf`, for a Viewer: the editor is not warmed.", "viewer", REALISTIC),
};
