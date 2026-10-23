// The design-parity scenarios of the shell (S0): the data of the Shell page of the handoff (`Lanekeeper Shell.dc.html`), so that
// the chrome of the app can be compared with the chrome of the design. The page's swimlane list is six lanes (sit, sitb, sitc,
// presita, presitb, presitc); the signed-in person is Priya Shah, an Admin, or Marco Diaz as a Viewer; the sidebar counts 11
// findings and 6 approvals (the design writes those two numbers into the sidebar; they are not the sum of the page's own rows,
// so the findings are spread to make the sum 11 here).
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { DocHit, Me, Proposal, SettingHit, SwimlaneSummary, User } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";

const TENANT = "11111111-1111-4111-8111-111111111111";
export const PRIYA = `${TENANT}:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa`;
export const MARCO = `${TENANT}:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb`;
const HASH = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";
const NOW = 1_760_000_000_000;

function lane(over: Partial<SwimlaneSummary> & Pick<SwimlaneSummary, "id" | "tenants" | "tier">): SwimlaneSummary {
  return {
    display_name: over.id,
    agent: "connected",
    last_scan_at: NOW,
    base_version: { tag: "v2.3.1", distance: 0, commit: COMMIT },
    drift_counts: [],
    finding_count: 0,
    project: over.tier === "PRESIT" ? "ops-presit" : "ops-sit",
    baseline: { state: "confirmed", by: null, at: null },
    ...over,
  };
}

export const SHELL_LANES: readonly SwimlaneSummary[] = [
  lane({ id: "sit", tenants: ["sit1"], tier: "SIT", drift_counts: [["nfs_ahead", 4]] }),
  lane({ id: "sitb", tenants: ["sit7"], tier: "SIT", drift_counts: [["nfs_ahead", 4]], finding_count: 6, base_version: { tag: "v2.3.1", distance: 4, commit: COMMIT } }),
  lane({ id: "sitc", tenants: ["sit9"], tier: "SIT", finding_count: 3, base_version: { tag: "v2.3.1", distance: 4, commit: COMMIT } }),
  lane({ id: "presita", tenants: ["sit11"], tier: "PRESIT", drift_counts: [["nfs_ahead", 3]], base_version: { tag: "v2.3.0", distance: 0, commit: COMMIT } }),
  lane({ id: "presitb", tenants: ["sit12"], tier: "PRESIT", agent: "disconnected", finding_count: 2, base_version: { tag: "v2.3.1", distance: 2, commit: COMMIT } }),
  lane({ id: "presitc", tenants: ["sit13"], tier: "PRESIT", drift_counts: [["nfs_ahead", 1]] }),
];

const proposal = (id: number): Proposal => ({
  id,
  author: MARCO,
  swimlane: "sitb",
  path: "tx-infinity-api/tx-infinity-core-sit1.yml",
  expected: { state: "hash", hash: HASH },
  new_hash: HASH,
  status: "pending",
  hunks: [],
  setting_changes: [],
  severity: null,
  created_at: NOW - 3_600_000,
  expires_at: NOW + 71 * 3_600_000,
  decided_by: null,
  decision_comment: null,
});

// The people the Admin's screens name: the signed-in Admin and the Viewer of the other Shell frame. A user is an id in the contract and
// the only list of names is the Admin-only user list (an Admin's screens ask for it, 500 at a time, to name who changed a file); a
// search (`q`, the palette's Users group, 8 at a time) is answered as the hub does, by a match on the name or the email. A slice that
// has people of its own (the Admin area, the approvals, the attribution frames) takes this response out of the base it starts from.
const person = (id: string, display_name: string, email: string, role: "admin" | "viewer"): User => ({ id, email, display_name, role, status: "active", requires_approval: false, github_login: null, last_seen_at: NOW });
export const SHELL_PEOPLE: readonly User[] = [person(PRIYA, "Priya Shah", "priya.shah@example.test", "admin"), person(MARCO, "Marco Diaz", "marco.diaz@example.test", "viewer")];
const peopleFor = (q: string | null): readonly User[] => {
  const wanted = q?.trim().toLowerCase() ?? "";
  return wanted === "" ? SHELL_PEOPLE : SHELL_PEOPLE.filter((u) => u.display_name.toLowerCase().includes(wanted) || u.email.toLowerCase().includes(wanted));
};
/** The Admin's user list, as the Shell's Admin answers it; a slice that builds its own base (the file view) adds this to it. */
export const SHELL_USERS: MockResponse = { method: "GET", path: "/api/v1/admin/users", body: (c: MockContext) => ({ items: peopleFor(c.query.get("q")), next: null }), sample: { query: new URLSearchParams("limit=500") } };

// What the palette's three searches answer, whatever the query (the design's palette shows "core" with files, docs and actions):
// three settings and two docs, so that a query that matches the six swimlanes too has more than nine results to number.
const SETTINGS: readonly SettingHit[] = [
  { swimlane: "sitb", file: "tx-infinity-api/tx-infinity-core-sit1.yml", path: "txInfinityOptions.enableAccountSorting", value: { text: "true", ty: "bool", flag: null }, location: { path: "txInfinityOptions.enableAccountSorting", lines: { start: 13, end: 13 } } },
  { swimlane: "sitc", file: "tx-infinity-api/tx-infinity-core-sit1.yml", path: "txInfinityOptions.remote-itm-teller.timeoutMs", value: { text: "30000", ty: "int", flag: null }, location: { path: "txInfinityOptions.remote-itm-teller.timeoutMs", lines: { start: 12, end: 12 } } },
  { swimlane: "sit", file: "tx-infinity-api/tx-infinity-core-sit1.yml", path: "logging.level", value: { text: "INFO", ty: "string", flag: null }, location: { path: "logging.level", lines: { start: 34, end: 34 } } },
];
const DOCS: readonly DocHit[] = [
  { doc: 1, title: "Account sorting", path: "/git/main/docs/remote-itm-teller.md", heading_path: ["remote-itm-teller", "Account sorting"], snippet: "Sorting is switched on by enableAccountSorting.", score: 0.9, related_files: [] },
  { doc: 2, title: "Tenant overrides", path: "/git/main/docs/tenants.md", heading_path: ["Tenants"], snippet: "A tenant file sits beside its base file.", score: 0.7, related_files: [] },
];

function scenario(id: string, description: string, me: Me): Scenario {
  const admin = me.role === "admin";
  return {
    id,
    description,
    responses: [
      { method: "GET", path: "/api/v1/me", body: me, headers: { "set-cookie": csrfCookie } },
      { method: "POST", path: "/auth/logout", status: 204 },
      { method: "GET", path: "/api/v1/swimlanes", body: { items: SHELL_LANES, next: null } },
      // Nobody has raised a pull request in the design's frames; the badges and the pull requests page have scenarios of their own.
      { method: "GET", path: "/api/v1/prs", body: { items: [], next: null } },
      { method: "GET", path: "/api/v1/search/settings", body: { items: SETTINGS, next: null } },
      { method: "GET", path: "/api/v1/docs/search", body: { hits: DOCS } },
      ...(admin
        ? [
            { method: "GET" as const, path: "/api/v1/proposals", body: { items: [1, 2, 3, 4, 5, 6].map(proposal), next: null } },
            // Nothing waits for an Admin, so the sidebar's Admin item has no count, as the design draws it.
            { method: "GET" as const, path: "/api/v1/admin/access-requests", body: { items: [], next: null } },
            SHELL_USERS,
          ]
        : []),
    ],
  };
}

export const scenarios: readonly Scenario[] = [
  scenario("design-shell-admin", "The Shell page of the design: Priya Shah, an Admin, six swimlanes, 11 findings and 6 pending approvals.", meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" } satisfies Partial<Me>)),
  scenario("design-shell-viewer", "The Shell page of the design for a Viewer: Marco Diaz, the same swimlanes, no Approvals or Audit.", meBody("viewer", { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test" } satisfies Partial<Me>)),
];
