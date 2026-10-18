// The design-parity scenarios of slice S8 (the Admin area): the data of the Admin page of the handoff (`Lanekeeper Admin Page.dc.html`
// and the board `Lanekeeper Admin`), for the sections this slice builds (users, swimlanes and agents, path mappings, merge modes,
// host tiers, folder mappings, notifications, MCP tokens). Sentinels, OS Login mapping, severity rules and retention are task T5's.
//
//  - The signed-in person is Priya Shah, an Admin, and the sidebar is the Shell page's (six swimlanes, 11 findings, 6 approvals): each
//    scenario starts from `design-shell-admin` and adds what the Admin area asks for. Every Admin screen asks for the pending
//    access requests (the count beside Users), so every scenario answers that.
//  - A state of a form (editing, a field in error) is not data: it is what a person does on the screen, so one scenario serves the
//    default, editing and error frames of a section. What differs in data is the empty list, and the answers of the hub that make a
//    state (a 409 on a user, a refusal on a save), which have scenarios of their own.
//  - The hub answers every write with the same body whatever was sent (a scenario is not a server), so a list does not change after a
//    write in the visual runs; the Playwright journeys that need that use a hub with state (e2e).
// Times are derived from the frozen clock the visual runs use (e2e/visual/render-references.mjs: 1_760_000_000_000).
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { problemBody } from "../../scenario.ts";
import { scenarios as shell, SHELL_LANES } from "../shell/index.ts";
import { channels, folders, hostTiers, IDS, mcpToken, mergeRules, NOW, pathRules, people, registrations, samRequest } from "./fixtures.ts";

const page = <T>(items: readonly T[]): { items: readonly T[]; next: null } => ({ items, next: null });
// The Shell's own answer to the pending requests is an empty list; the harness answers with the FIRST response that matches, so it
// must not be here or it hides the page's own (Sam Okafor's request would never be drawn).
const shellBase = (id: "design-shell-admin" | "design-shell-viewer"): readonly MockResponse[] => (shell.find((s) => s.id === id)?.responses ?? []).filter((r) => !(r.method === "GET" && r.path === "/api/v1/admin/access-requests"));

const list = (path: string, items: readonly unknown[]): MockResponse => ({ method: "GET", path, body: page(items) });
const echo = (method: "PUT" | "POST", path: string, sample: Partial<MockContext>, extra: (c: MockContext) => object = () => ({}), status = 200): MockResponse => ({
  method,
  path,
  status,
  body: (c: MockContext) => ({ id: c.params["id"] ?? "x", ...(typeof c.body === "object" && c.body !== null ? c.body : {}), ...extra(c) }),
  sample,
});
const noContent = (path: string): MockResponse => ({ method: "DELETE", path, status: 204 });

/** What every Admin screen asks for besides its own data: the people waiting (the count beside Users). */
const requests = (items: readonly unknown[] = [samRequest()]): MockResponse => list("/api/v1/admin/access-requests", items);

function admin(id: string, description: string, ...own: MockResponse[]): Scenario {
  return { id, description, responses: [...shellBase("design-shell-admin"), requests(), ...own] };
}

const userAnswer = { method: "PATCH", path: "/api/v1/admin/users/{id}", body: (c: MockContext) => ({ ...(people().find((u) => u.id === c.params["id"]) ?? people()[2]), ...(typeof c.body === "object" && c.body !== null ? c.body : {}) }), sample: { params: { id: IDS.lena }, body: { role: "admin" } } } satisfies MockResponse;

const SETTINGS: readonly MockResponse[] = [
  {
    method: "GET",
    path: "/api/v1/admin/swimlanes/{id}/tenants",
    body: (c: MockContext) => ({ tenants: SHELL_LANES.find((l) => l.id === c.params["id"])?.tenants ?? ["sit1"] }),
    sample: { params: { id: "sitb" } },
  },
  { method: "PUT", path: "/api/v1/admin/swimlanes/{id}/tenants", body: (c: MockContext) => ({ tenants: (c.body as { tenants?: string[] } | undefined)?.tenants ?? ["sit1"] }), sample: { params: { id: "sitb" }, body: { tenants: ["sit7"] } } },
  { method: "GET", path: "/api/v1/admin/swimlanes/{id}/auto-notify", body: (c: MockContext) => ({ enabled: c.params["id"] === "sitb" }), sample: { params: { id: "sitb" } } },
  { method: "PUT", path: "/api/v1/admin/swimlanes/{id}/auto-notify", body: (c: MockContext) => ({ enabled: (c.body as { enabled?: boolean } | undefined)?.enabled ?? false }), sample: { params: { id: "sitb" }, body: { enabled: true } } },
  {
    method: "PUT",
    path: "/api/v1/admin/agents/{swimlane}",
    body: (c: MockContext) => ({
      swimlane: c.params["swimlane"] ?? "sitb",
      service_account_email: (c.body as { service_account_email?: string } | undefined)?.service_account_email ?? "lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com",
      status: "connected",
      connected_replica: null,
      last_seen_at: NOW,
      certificate_expires_at: null,
    }),
    sample: { params: { swimlane: "sitb" }, body: { service_account_email: "lanekeeper-agent@dfb-nonprod-sit.iam.gserviceaccount.com" } },
  },
  noContent("/api/v1/admin/agents/{swimlane}"),
  {
    method: "POST",
    path: "/api/v1/admin/agents/{swimlane}/join-token",
    status: 201,
    body: (c: MockContext) => ({ token: "lkjt_7f3a9c21e8b04d5a96c1f2e07b38d4a5", swimlane: c.params["swimlane"] ?? "sitc", expires_at: NOW + 24 * 3_600_000 }),
    sample: { params: { swimlane: "sitc" } },
  },
];

const RULE_PUTS = (path: string, sampleBody: object): MockResponse => echo("PUT", path, { params: { id: "rule" }, body: sampleBody });

export const scenarios: readonly Scenario[] = [
  admin("design-admin-users", "Users and access requests (frames 1a, 1c, 1d, 1e): seven people, Priya Shah (you) among them, and Sam Okafor's request for Editor.", list("/api/v1/admin/users", people()), userAnswer, {
    method: "POST",
    path: "/api/v1/admin/access-requests/{id}/approve",
    body: { id: IDS.sam, email: "sam.okafor@corp.example", display_name: "Sam Okafor", role: "editor", status: "active", requires_approval: false, github_login: "samokafor", last_seen_at: NOW },
  }, {
    method: "POST",
    path: "/api/v1/admin/access-requests/{id}/reject",
    body: samRequest({ status: "rejected", decided_by: IDS.priya, decided_at: NOW }),
  }),
  {
    id: "design-admin-users-empty",
    description: "Users with no request waiting (frame 1b): the empty state under Pending requests.",
    responses: [...shellBase("design-shell-admin"), requests([]), list("/api/v1/admin/users", people())],
  },
  admin("design-admin-users-conflict", "Users, state C03: the hub answers a role change with 409 because the person changed meanwhile.", list("/api/v1/admin/users", people()), {
    method: "PATCH",
    path: "/api/v1/admin/users/{id}",
    status: 409,
    body: problemBody(409, "conflict", "Conflict"),
  }),
  admin("design-admin-swimlanes", "Swimlanes and agents (frames 1f and 1j): six swimlanes, an agent for each (presitb offline, presitc never seen), settings and a join token.", list("/api/v1/admin/agents", registrations()), ...SETTINGS),
  {
    id: "design-admin-swimlanes-empty",
    description: "Swimlanes and agents with no swimlane (frame 1g): the hub knows none, and nothing here can register one (CG15).",
    responses: [
      ...shellBase("design-shell-admin").filter((r) => r.path !== "/api/v1/swimlanes"),
      { method: "GET", path: "/api/v1/swimlanes", body: page([]) },
      requests(),
      list("/api/v1/admin/agents", []),
    ],
  },
  admin("design-admin-paths", "Path mappings (frames 1k to 1o): the default rule and one for two swimlanes.", list("/api/v1/admin/path-mappings", pathRules()), RULE_PUTS("/api/v1/admin/path-mappings/{id}", { base_root: "config", tenant_root: "data/config", swimlanes: [] }), noContent("/api/v1/admin/path-mappings/{id}")),
  admin("design-admin-paths-empty", "Path mappings with no rule (frame 1l).", list("/api/v1/admin/path-mappings", []), RULE_PUTS("/api/v1/admin/path-mappings/{id}", { base_root: "config", tenant_root: "data/config", swimlanes: [] })),
  admin("design-admin-merge", "Merge modes (frames 1p to 1t): security merged, services and images whole file.", list("/api/v1/admin/merge-modes", mergeRules()), RULE_PUTS("/api/v1/admin/merge-modes/{id}", { file_pattern: "security/**", mode: "spring_merge" }), noContent("/api/v1/admin/merge-modes/{id}")),
  admin("design-admin-merge-empty", "Merge modes with no rule (frame 1q).", list("/api/v1/admin/merge-modes", []), RULE_PUTS("/api/v1/admin/merge-modes/{id}", { file_pattern: "security/**", mode: "spring_merge" })),
  admin("design-admin-tiers", "Host tiers (frames 1u to 1y): dev, uat and prod patterns.", list("/api/v1/admin/host-tiers", hostTiers()), RULE_PUTS("/api/v1/admin/host-tiers/{id}", { host_pattern: "*-dev.example.net", tier: "dev" }), noContent("/api/v1/admin/host-tiers/{id}")),
  admin("design-admin-tiers-empty", "Host tiers with no pattern (frame 1v).", list("/api/v1/admin/host-tiers", []), RULE_PUTS("/api/v1/admin/host-tiers/{id}", { host_pattern: "*-dev.example.net", tier: "dev" })),
  admin("design-admin-folders", "Folder mappings (frames 1z to 1ad): three folders and the Deployments that read them.", list("/api/v1/admin/folder-mappings", folders()), RULE_PUTS("/api/v1/admin/folder-mappings/{id}", { folder: "video", deployments: [{ namespace: "sitb", name: "video-service" }] }), noContent("/api/v1/admin/folder-mappings/{id}")),
  admin("design-admin-folders-empty", "Folder mappings with none (frame 1aa).", list("/api/v1/admin/folder-mappings", []), RULE_PUTS("/api/v1/admin/folder-mappings/{id}", { folder: "video", deployments: [{ namespace: "sitb", name: "video-service" }] })),
  admin(
    "design-admin-notify",
    "Notifications (frames 1ae to 1ai): three Teams channels, two with a webhook set (never shown) and one off.",
    list("/api/v1/admin/notifications", channels()),
    echo("PUT", "/api/v1/admin/notifications/{id}", { params: { id: "approvals" }, body: { name: "Approvals", min_severity: "low", kinds: ["proposal_pending"], enabled: true } }, () => ({ webhook_configured: true })),
    noContent("/api/v1/admin/notifications/{id}"),
  ),
  admin("design-admin-notify-empty", "Notifications with no channel (frame 1af): notifications are off.", list("/api/v1/admin/notifications", []), echo("PUT", "/api/v1/admin/notifications/{id}", { params: { id: "approvals" }, body: { name: "Approvals", min_severity: "low", kinds: ["proposal_pending"], enabled: true } }, () => ({ webhook_configured: true }))),
  admin(
    "design-admin-mcp",
    "MCP personal tokens (frames 1aj to 1an): the Admin's own tokens, one active and one expired.",
    list("/api/v1/me/mcp-tokens", [mcpToken(1, "cursor-priya"), mcpToken(2, "old-laptop", { created_at: NOW - 40 * 86_400_000, expires_at: NOW - 33 * 86_400_000, last_used_at: NOW - 34 * 86_400_000 })]),
  ),
  admin("design-admin-mcp-empty", "MCP personal tokens with none issued (frame 1ak).", list("/api/v1/me/mcp-tokens", [])),
  admin("design-admin-mcp-off", "MCP mode off (the design's mcp=false): the hub answers the token list with 404.", { method: "GET", path: "/api/v1/me/mcp-tokens", status: 404, body: problemBody(404, "not_found", "Not found") }),
  {
    id: "design-admin-viewer",
    description: "The Admin area for a Viewer (state P02): Marco Diaz is shown the permission state, and the hub is asked for nothing of the Admin area.",
    responses: [...shellBase("design-shell-viewer")],
  },
];
