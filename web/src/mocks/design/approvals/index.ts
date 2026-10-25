// The design-parity scenarios of slice S7 (approvals and audit): the data of `Lanekeeper Approvals Page.dc.html` and
// `Lanekeeper Audit Page.dc.html`, so that the app's screens can be compared with the design's. The signed-in person is Priya
// Shah, an Admin; the proposals are the page's (AP-2048 arrives live and is in its own scenario); the audit events are the page's
// (a whole chain, and one with a link cut). The shell around the page is the Shell scenario's (its swimlanes, its sidebar).
// Loading and error are not scenarios: the harness holds or fails the request (`page.route`), as the other slices' do.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { AuditEvent, Me, Proposal, Role, User } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { scenarios as shellScenarios } from "../shell/index.ts";
import { ALL, ARRIVAL, AUDIT, chainHash, hash, NOW, PEOPLE, PRIYA } from "./fixtures.ts";

const PROPOSALS = "/api/v1/proposals";
const query = (text: string): Partial<MockContext> => ({ query: new URLSearchParams(text) });
const isDecided = (p: Proposal): boolean => p.status === "approved" || p.status === "applied" || p.status === "rejected";
const newestFirst = <T extends { id: number }>(items: readonly T[]): T[] => [...items].sort((a, b) => b.id - a.id);

/** The shell of the app around the page: who is signed in, the swimlanes, the search the palette uses. Not the Shell's own proposals. */
function shell(role: Role): readonly MockResponse[] {
  const base = shellScenarios.find((s) => s.id === (role === "admin" ? "design-shell-admin" : "design-shell-viewer"));
  return (base?.responses ?? []).filter((r) => r.path !== PROPOSALS && r.path !== "/api/v1/me" && r.path !== "/api/v1/admin/users");
}

function me(role: Role): MockResponse {
  const priya: Partial<Me> = { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" };
  return { method: "GET", path: "/api/v1/me", body: meBody(role, priya), headers: { "set-cookie": "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax" } };
}

const users: MockResponse = { method: "GET", path: "/api/v1/admin/users", body: { items: PEOPLE satisfies readonly User[], next: null } };

/** The list the hub answers for a question: a status, `decided=true`, or neither. */
function listBody(rows: readonly Proposal[]) {
  return ({ query: q }: MockContext) => {
    const status = q.get("status");
    const decided = q.get("decided") === "true";
    return { items: newestFirst(rows.filter((p) => (status === null || p.status === status) && (!decided || isDecided(p)))), next: null };
  };
}

function proposals(rows: readonly Proposal[]): readonly MockResponse[] {
  const find = (id: string): Proposal => rows.find((p) => String(p.id) === id) ?? rows[0] ?? ALL[0] ?? ARRIVAL;
  return [
    { method: "GET", path: PROPOSALS, body: listBody(rows), sample: query("status=pending&limit=100"), samples: [query("status=stale&limit=100"), query("status=expired&limit=100"), query("decided=true&limit=100")] },
    { method: "GET", path: `${PROPOSALS}/{id}`, body: ({ params }: MockContext) => find(params["id"] ?? ""), sample: { params: { id: "2046" } } },
    { method: "POST", path: `${PROPOSALS}/{id}/approve`, body: ({ params }: MockContext) => ({ outcome: "applied", hash: find(params["id"] ?? "").new_hash, audit_id: 18470 }), sample: { params: { id: "2046" } } },
    {
      method: "POST",
      path: `${PROPOSALS}/{id}/reject`,
      body: ({ params, body }: MockContext): Proposal => {
        const reason = typeof body === "object" && body !== null && "reason" in body && typeof body.reason === "string" ? body.reason : "Not now.";
        return { ...find(params["id"] ?? ""), status: "rejected", decided_by: PRIYA, decided_at: NOW, decision_comment: reason };
      },
      sample: { params: { id: "2046" }, body: { reason: "Raise maxRetries to 4 first." } },
    },
  ];
}

function approvals(id: string, description: string, role: Role, rows: readonly Proposal[], extra: readonly MockResponse[] = []): Scenario {
  // An override goes first: the harness answers with the first response that matches.
  return { id, description, responses: [...extra, me(role), ...shell(role), users, ...proposals(rows)] };
}

/** The queue of the page without the proposal that arrives. */
const QUEUE: readonly Proposal[] = ALL;

/** The audit read: the filters the hub takes, applied to the events. */
function auditBody(events: readonly AuditEvent[]) {
  return ({ query: q }: MockContext) => {
    const rows = events.filter((e) => (q.get("actor") === null || e.actor === q.get("actor")) && (q.get("action") === null || e.action === q.get("action")) && (q.get("swimlane") === null || e.swimlane === q.get("swimlane")));
    return { items: [...rows].sort((a, b) => b.seq - a.seq), next: null };
  };
}

function audit(id: string, description: string, events: readonly AuditEvent[]): Scenario {
  const read: MockResponse = { method: "GET", path: "/api/v1/admin/audit", body: auditBody(events), sample: query("limit=100"), samples: [query("swimlane=sitb&limit=100")] };
  return { id, description, responses: [me("admin"), ...shell("admin"), users, read] };
}

/** The same chain with the link of event 18,455 cut: it stores a hash that is not the hash of 18,454. */
const BROKEN: readonly AuditEvent[] = AUDIT.map((e) => (e.seq === 18455 ? { ...e, prev_hash: chainHash(18448) } : e));

const lock = (status: 409 | 423): MockResponse => ({
  method: "POST",
  path: `${PROPOSALS}/{id}/approve`,
  status,
  body: status === 409 ? problemBody(409, "conflict", "Conflict", { current: hash("e2a8f610") }) : problemBody(423, "locked", "Locked", { until: NOW + 3_600_000 }),
});

export const scenarios: readonly Scenario[] = [
  approvals("design-approvals-pending", "The Approvals page, Pending: six waiting, two stale, two expired, three decided; Priya Shah is an Admin and has one proposal of her own (AP-2044).", "admin", QUEUE),
  approvals("design-approvals-arrival", "The Approvals page with AP-2048 in the queue: the proposal that arrives while the page is open (frame 1s), by Dana Okoye through Cursor.", "admin", [ARRIVAL, ...QUEUE]),
  approvals("design-approvals-empty", "The Approvals page with nothing in any list (frame 1r).", "admin", []),
  approvals("design-approvals-editor", "Approvals needs Admin: an Editor is shown the permission screen (frame 1p).", "editor", QUEUE),
  approvals("design-approvals-conflict", "Approving answers 409: the file changed first.", "admin", QUEUE, [lock(409)]),
  approvals("design-approvals-locked", "Approving answers 423: the swimlane is locked.", "admin", QUEUE, [lock(423)]),
  audit("design-approvals-audit", "The Audit log, default: fourteen events, the chain whole, quick filters for Cursor and unknown author.", AUDIT),
  audit("design-approvals-audit-empty", "The Audit log with no events.", []),
  audit("design-approvals-audit-broken", "The Audit log with a cut link at event 18,455 (the design's frame 1x, on this data).", BROKEN),
];
