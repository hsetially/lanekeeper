// A small hub for the tests of the swimlane screens: a world (who is signed in, the swimlanes, their services, files, findings and
// audit rows) and the function that answers `fetch` for it. The tests change the world between two answers to play what a live
// event announces. Test code only: nothing outside the tests imports it.
import type { AuditEvent, FileEntry, Finding, Role, ServiceState, SwimlaneDetail, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { json, me, problem, type Api, type Call } from "../../../test/app.tsx";
import { detailOf } from "./fixtures.ts";

export type World = {
  role: Role | null;
  lanes: SwimlaneSummary[];
  /** Services by swimlane id; a swimlane with none has none. */
  services: Record<string, ServiceState[]>;
  auto_notify: Record<string, boolean>;
  /** The drift listing by swimlane id (all states). */
  drift: Record<string, FileEntry[]>;
  /** The tree by swimlane id and folder (`""` is the root). */
  tree: Record<string, Record<string, FileEntry[]>>;
  findings: Finding[];
  audit: AuditEvent[];
  /** What `POST /restart` answers: a function of the service asked for, or a fixed response. */
  restart: Response | ((body: { namespace: string; name: string }, call: Call) => Response);
  /** When set, the swimlane list answers this problem instead. */
  listProblem: Response | null;
};

export function newWorld(role: Role | null, lanes: SwimlaneSummary[] = []): World {
  return { role, lanes, services: {}, auto_notify: {}, drift: {}, tree: {}, findings: [], audit: [], restart: json({ outcome: "applied", hash: null, audit_id: 7 }), listProblem: null };
}

const notFound = (): Response => problem(404, "not_found");

/** The detail of a swimlane in the world, or undefined when there is none. */
export function detailIn(world: World, id: string): SwimlaneDetail | undefined {
  const summary = world.lanes.find((lane) => lane.id === id);
  return summary === undefined ? undefined : detailOf(summary, world.services[id] ?? [], world.auto_notify[id] ?? false);
}

export function worldApi(world: World): Api {
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    const path = url.pathname;
    if (path === "/api/v1/me") return json(me(world.role));
    if (path === "/api/v1/swimlanes") return world.listProblem ?? json({ items: world.lanes, next: null });
    if (path === "/api/v1/proposals") return json({ items: [], next: null });
    if (path === "/api/v1/findings") {
      const swimlane = url.searchParams.get("swimlane");
      return json({ items: world.findings.filter((f) => swimlane === null || f.swimlane === swimlane), next: null });
    }
    if (path === "/api/v1/admin/audit") {
      const swimlane = url.searchParams.get("swimlane");
      return json({ items: world.audit.filter((e) => swimlane === null || e.swimlane === swimlane), next: null });
    }
    const swimlane = /^\/api\/v1\/swimlanes\/([^/]+)(?:\/(.+))?$/.exec(path);
    if (swimlane) {
      const id = decodeURIComponent(swimlane[1] ?? "");
      const rest = swimlane[2] ?? "";
      if (rest === "") return detailIn(world, id) === undefined ? notFound() : json(detailIn(world, id));
      if (!world.lanes.some((lane) => lane.id === id)) return notFound();
      if (rest === "drift") {
        const state = url.searchParams.get("state");
        return json({ items: (world.drift[id] ?? []).filter((f) => state === null || f.drift === state), next: null });
      }
      if (rest === "tree") {
        const prefix = url.searchParams.get("prefix") ?? "";
        return json({ items: world.tree[id]?.[prefix] ?? [], next: null });
      }
      if (rest === "restart" && call.method === "POST") {
        const body = JSON.parse(call.body ?? "{}") as { namespace: string; name: string };
        return typeof world.restart === "function" ? world.restart(body, call) : world.restart.clone();
      }
    }
    return notFound();
  };
}
