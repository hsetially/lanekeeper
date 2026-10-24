// A small hub for the tests of the approvals and audit screens: a world (who is signed in, the people, the swimlanes, the
// proposals, the audit events) and the function that answers `fetch` for it, the way the hub's own routes do (a newest-first keyset
// page, the `status` and `decided` filters, a decision that changes the proposal). The tests change the world between two answers
// to play what a live event announces. Test code only: nothing outside the tests imports it.
import type { AuditEvent, Me, Proposal, Role, SwimlaneSummary, User } from "../../../api/generated/model/index.ts";
import { json, me, problem, type Api, type Call } from "../../../test/app.tsx";
import { ALL, NOW, PEOPLE, PRIYA } from "./fixtures.ts";

export type World = {
  role: Role | null;
  /** Who is signed in. */
  self: Pick<Me, "id" | "display_name">;
  users: User[];
  lanes: SwimlaneSummary[];
  proposals: Proposal[];
  audit: AuditEvent[];
  /** The hub's page: it answers at most this many whatever limit is asked for (25 today). */
  pageSize: number;
  /** When set, the list answers this instead (a problem), whatever it was asked for. */
  listProblem: Response | null;
  /** Like `listProblem`, but only for the Decided query (`decided=true`), which the hub does not accept yet. */
  decidedProblem: Response | null;
  /** What `POST /approve` answers: a function of the proposal, or a fixed response. The default applies it. */
  approve: Response | ((proposal: Proposal, call: Call) => Response) | null;
  reject: Response | ((proposal: Proposal, call: Call, reason: string) => Response) | null;
  /** When set, the audit read answers this instead. */
  auditProblem: Response | null;
  /** Every decision the world was asked to record, with the headers and body it came with. */
  decisions: Array<{ id: number; verb: "approve" | "reject"; headers: Headers; body: string | undefined }>;
};

export function newWorld(role: Role | null = "admin", proposals: readonly Proposal[] = ALL): World {
  return {
    role,
    self: { id: PRIYA, display_name: "Priya Shah" },
    users: [...PEOPLE],
    lanes: [],
    proposals: [...proposals],
    audit: [],
    pageSize: 25,
    listProblem: null,
    decidedProblem: null,
    approve: null,
    reject: null,
    auditProblem: null,
    decisions: [],
  };
}

const notFound = (): Response => problem(404, "not_found");
const isDecided = (p: Proposal): boolean => p.status === "approved" || p.status === "applied" || p.status === "rejected";

/** A page of `items` (newest first by `keyOf`), cut at the hub's own page size, with the key of the last as the cursor. */
function page<T>(items: readonly T[], keyOf: (item: T) => number, url: URL, pageSize: number): Response {
  const limit = Math.min(Number(url.searchParams.get("limit") ?? pageSize), pageSize);
  const below = url.searchParams.get("cursor");
  const ordered = [...items].sort((a, b) => keyOf(b) - keyOf(a)).filter((item) => below === null || keyOf(item) < Number(below));
  const shown = ordered.slice(0, limit);
  const last = shown[shown.length - 1];
  const next = ordered.length > shown.length && last !== undefined ? String(keyOf(last)) : null;
  return json({ items: shown, next });
}

export function worldApi(world: World): Api {
  return (call) => {
    const url = new URL(call.path, "http://hub.test");
    const path = url.pathname;
    if (path === "/api/v1/me") return json(me(world.role, { ...world.self } satisfies Partial<Me>));
    if (path === "/api/v1/swimlanes") return json({ items: world.lanes, next: null });
    if (path === "/api/v1/admin/users") return json({ items: world.users, next: null });
    if (path === "/api/v1/admin/audit") {
      if (world.auditProblem) return world.auditProblem.clone();
      const q = url.searchParams;
      const rows = world.audit.filter(
        (e) =>
          (q.get("actor") === null || e.actor === q.get("actor")) &&
          (q.get("action") === null || e.action === q.get("action")) &&
          (q.get("swimlane") === null || e.swimlane === q.get("swimlane")) &&
          (q.get("from") === null || e.at >= Number(q.get("from"))) &&
          (q.get("to") === null || e.at <= Number(q.get("to"))),
      );
      return page(rows, (e) => e.seq, url, world.pageSize);
    }
    if (path === "/api/v1/proposals") {
      const decided = url.searchParams.get("decided") === "true";
      if (decided && world.decidedProblem) return world.decidedProblem.clone();
      if (world.listProblem) return world.listProblem.clone();
      const status = url.searchParams.get("status");
      const swimlane = url.searchParams.get("swimlane");
      const rows = world.proposals.filter((p) => (status === null || p.status === status) && (!decided || isDecided(p)) && (swimlane === null || p.swimlane === swimlane));
      return page(rows, (p) => p.id, url, world.pageSize);
    }
    const one = /^\/api\/v1\/proposals\/(\d+)(?:\/(approve|reject))?$/.exec(path);
    if (one) {
      const id = Number(one[1]);
      const found = world.proposals.find((p) => p.id === id);
      if (found === undefined) return notFound();
      const verb = one[2];
      if (verb === undefined) return json(found);
      if (call.method !== "POST") return problem(405, "method_not_allowed");
      world.decisions.push({ id, verb: verb as "approve" | "reject", headers: call.headers, body: call.body });
      if (verb === "approve") {
        if (world.approve !== null) return typeof world.approve === "function" ? world.approve(found, call) : world.approve.clone();
        world.proposals = world.proposals.map((p) => (p.id === id ? { ...p, status: "applied", decided_by: world.self.id, decided_at: NOW } : p));
        return json({ outcome: "applied", hash: found.new_hash, audit_id: 18470 });
      }
      const reason = (JSON.parse(call.body ?? "{}") as { reason?: string }).reason ?? "";
      if (world.reject !== null) return typeof world.reject === "function" ? world.reject(found, call, reason) : world.reject.clone();
      const rejected: Proposal = { ...found, status: "rejected", decided_by: world.self.id, decided_at: NOW, decision_comment: reason };
      world.proposals = world.proposals.map((p) => (p.id === id ? rejected : p));
      return json(rejected);
    }
    return notFound();
  };
}
