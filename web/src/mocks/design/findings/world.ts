// A small hub for the tests of the Findings screen (src/features/findings/*.test.tsx): a world (who is signed in, the swimlanes, the findings) and the function that
// answers `fetch` for it, with the filters, the order and the cursor of the registry. A test changes the world between two
// answers to play what a live event announces. Test code only: nothing outside the tests imports it.
import type { Finding, Role, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { json, me, problem, type Api } from "../../../test/app.tsx";
import { laneCounts } from "./fixtures.ts";
import { findingPage } from "./server.ts";

export type World = {
  role: Role | null;
  lanes: SwimlaneSummary[];
  findings: Finding[];
  /** When set, the findings list answers this instead. */
  findingsAnswer: (() => Response | Promise<Response>) | null;
  /** When set, the swimlane list answers this problem instead. */
  lanesProblem: Response | null;
};

export function newWorld(findings: readonly Finding[], role: Role | null = "viewer", lanes?: SwimlaneSummary[]): World {
  return { role, lanes: lanes ?? laneCounts(findings), findings: [...findings], findingsAnswer: null, lanesProblem: null };
}

export const pathOf = (path: string): string => new URL(path, "http://hub.test").pathname;
export const isFindings = (path: string): boolean => pathOf(path) === "/api/v1/findings";
export const queryOf = (path: string): URLSearchParams => new URL(path, "http://hub.test").searchParams;

export function worldApi(world: World): Api {
  return (call) => {
    const path = pathOf(call.path);
    if (path === "/api/v1/me") return json(me(world.role));
    if (path === "/api/v1/swimlanes") return world.lanesProblem?.clone() ?? json({ items: world.lanes, next: null });
    if (path === "/api/v1/proposals") return json({ items: [], next: null });
    if (path === "/api/v1/findings") return world.findingsAnswer?.() ?? json(findingPage(world.findings, queryOf(call.path)));
    return problem(404, "not_found");
  };
}
