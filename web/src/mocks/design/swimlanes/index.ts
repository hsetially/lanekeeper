// The design-parity scenarios of slice S2 (the swimlane list, the detail tabs, services, activity): the data of the Swimlanes board
// (`Lanekeeper Shell.dc.html` with page=swimlanes) and of the Swimlane Detail board (page=detail), so that the app can be compared
// with the reference frames. The six swimlanes are the Shell's (SHELL_LANES, which keeps the sidebar's 11 findings), with the drift,
// pickup, versions, agents and times of the Swimlanes page. Times are offsets from the frozen clock the visual runs use
// (1_760_000_000_000), so "2 h ago" reads the same every time.
//
// Where the contract gives the design less than it draws, the scenario gives what the contract carries and no more: an agent's
// "last seen" is its last scan, a service has no pod count, a finding has no author (design/DEVIATIONS.md).
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { AuditEvent, DriftState, FileEntry, Finding, FindingPage, ServiceState, SwimlaneDetail, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { MARCO, PRIYA, SHELL_LANES, scenarios as shellScenarios } from "../shell/index.ts";
import { auditEvent, detailOf, fileEntry, finding, folderEntry, lane, service } from "./fixtures.ts";

const NOW = 1_760_000_000_000;
const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const COMMIT = "a3f9c21a3f9c21a3f9c21a3f9c21a3f9c21a3f9c";
const version = (tag: string, distance: number) => ({ tag, distance, commit: COMMIT });

/** What the Swimlanes page draws for each of the Shell's six swimlanes. */
const SIX: readonly SwimlaneSummary[] = SHELL_LANES.map((base): SwimlaneSummary => {
  switch (base.id) {
    case "sit":
      return { ...base, drift_counts: [], last_scan_at: NOW - 2 * HOUR, base_version: version("v2.3.1", 0) };
    case "sitb":
      return { ...base, drift_counts: [["conflict", 1], ["nfs_ahead", 2], ["git_ahead", 1]], pickup_counts: [["needs_notify_or_restart", 2]], last_scan_at: NOW - 2_000, base_version: version("v2.3.1", 4) };
    case "sitc":
      return { ...base, drift_counts: [["nfs_ahead", 1]], last_scan_at: NOW - 31 * MIN, base_version: version("v2.3.1", 4) };
    case "presita":
      return { ...base, drift_counts: [["git_ahead", 2]], last_scan_at: NOW - DAY, base_version: version("v2.3.0", 0) };
    case "presitb":
      return { ...base, drift_counts: [["nfs_ahead", 3]], last_scan_at: NOW - 14 * MIN, base_version: version("v2.3.1", 2) };
    default:
      return { ...base, drift_counts: [["unknown", 412]], last_scan_at: NOW - 6 * DAY, base_version: null, baseline: { state: "unconfirmed", by: null, at: null } };
  }
});

/** The 40-swimlane density test of the Swimlanes board: the six, and 34 more generated the way the board generates them. */
function forty(): SwimlaneSummary[] {
  const letters = "defghijklmnopqrstuvwxyz".split("");
  const versions = [version("v2.3.1", 4), version("v2.3.1", 0), version("v2.3.0", 1), version("v2.2.9", 0)];
  const lasts = [4 * MIN, 12 * MIN, HOUR, 3 * HOUR, DAY, 2 * DAY, 5 * DAY];
  const more = Array.from({ length: 34 }, (_, i): SwimlaneSummary => {
    const pre = i >= 23;
    const id = (pre ? "presit" : "sit") + ((pre ? "defghijklmn"[i - 23] : letters[i]) ?? "");
    const s = ((i * 9301 + 49297) % 233280) / 233280;
    const drift: Array<[DriftState, number]> = [];
    if (s > 0.9) drift.push(["conflict", 1 + (i % 2)]);
    if (s > 0.5) drift.push(["nfs_ahead", Math.ceil(s * 6)]);
    if (i % 3 === 0) drift.push(["git_ahead", 1 + (i % 12)]);
    return lane(id, {
      tier: pre ? "PRESIT" : "SIT",
      project: pre ? "ops-presit" : i % 3 === 1 ? "cards-sit" : "ops-sit",
      tenants: [`sit${(i % 13) + 1}`],
      base_version: versions[i % 4] ?? null,
      agent: i === 17 ? "disconnected" : "connected",
      drift_counts: drift,
      finding_count: i % 4 === 0 ? i % 7 : 0,
      last_scan_at: NOW - (lasts[i % 7] ?? HOUR),
      baseline: i === 21 || i === 30 ? { state: "unconfirmed", by: null, at: null } : lane(id).baseline ?? null,
    });
  });
  return [...SIX, ...more];
}

// ---- sitb, the swimlane of the Swimlane Detail board ----

const SITB_SERVICES: ServiceState[] = [
  service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 3 * MIN, pending_files: ["tx-infinity-api/tx-infinity-core-sit7.yml", "application.properties"] }),
  service("ui-service", { pickup: "needs_notify_or_restart", pending_since: NOW - 25 * MIN, pending_files: ["application.properties"] }),
  service("document-service"),
  service("holds-service"),
];

/** The folders of the swimlane, by prefix. The tree opens the root, and a folder when it is opened. */
const SITB_TREE: Record<string, FileEntry[]> = {
  "": [
    folderEntry("tx-infinity-api/"),
    folderEntry("ui/remote-itm-teller/"),
    folderEntry("limits/"),
    folderEntry("security/"),
    folderEntry("document-service/resources/"),
    folderEntry("ej-templates/"),
    fileEntry("application.properties", { drift: "nfs_ahead", logical_file: "application.properties", pickup_state: "needs_notify_or_restart" }),
    fileEntry("channels.yml", { logical_file: "channels.yml" }),
  ],
  "tx-infinity-api": [
    fileEntry("tx-infinity-api/tx-infinity-core.yml", { logical_file: "tx-infinity-api/tx-infinity-core.yml" }),
    fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "tx-infinity-api/tx-infinity-core.yml", drift: "nfs_ahead", pickup_state: "needs_notify_or_restart" }),
    fileEntry("tx-infinity-api/tx-infinity-gateway.yml", { logical_file: "tx-infinity-api/tx-infinity-gateway.yml" }),
    fileEntry("tx-infinity-api/account-sorting-config.yml", { logical_file: "tx-infinity-api/account-sorting-config.yml" }),
    fileEntry("tx-infinity-api/account-sorting-config-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "tx-infinity-api/account-sorting-config.yml" }),
  ],
  "ui/remote-itm-teller": [fileEntry("ui/remote-itm-teller/teller-ui.yml", { drift: "git_ahead" }), fileEntry("ui/remote-itm-teller/teller-labels.json", {})],
  limits: [fileEntry("limits/limits.yml", { drift: "conflict" }), fileEntry("limits/channel-limits.yml")],
  security: [fileEntry("security/security-roles.yml"), fileEntry("security/security-policy.yml")],
};

const SITB_DRIFT: FileEntry[] = [
  fileEntry("limits/limits.yml", { drift: "conflict" }),
  fileEntry("application.properties", { drift: "nfs_ahead" }),
  fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { kind: "tenant", tenant: "sit7", drift: "nfs_ahead", logical_file: "tx-infinity-api/tx-infinity-core.yml" }),
  fileEntry("ui/remote-itm-teller/teller-ui.yml", { drift: "git_ahead" }),
  fileEntry("document-service/resources/receipt-logo.jpeg", { drift: "untracked", class: "binary" }),
];

const FINDINGS: Finding[] = [
  finding(1, { kind: "duplicate_keys", severity: "high", path: "security/security-roles.yml", message: "entryGroupConfigMap.atm-iso.DEPOSIT.transactionKey is set twice; the second value wins.", locations: [{ path: "entryGroupConfigMap.atm-iso.DEPOSIT.transactionKey", lines: { start: 212, end: 212 } }, { path: "entryGroupConfigMap.atm-iso.DEPOSIT.transactionKey", lines: { start: 488, end: 488 } }] }),
  finding(2, { kind: "redundant_tenant_file", severity: "low", path: "tx-infinity-api/account-sorting-config-sit7.yml", message: "Identical to account-sorting-config.yml, so the tenant file changes nothing.", locations: [] }),
  finding(3, { swimlane: "sitc", kind: "drift", severity: "medium", path: "tx-infinity-api/tx-infinity-core.yml", message: "The file differs between NFS and Git.", locations: [] }),
];

const AUDIT: AuditEvent[] = [
  auditEvent(6, { at: NOW - 2_000, action: "file.observed", target: "limits/limits.yml", via: "system", actor: null }),
  auditEvent(5, { at: NOW - 3 * MIN, action: "file.observed", target: "tx-infinity-api/tx-infinity-core-sit7.yml", via: "system", actor: null }),
  auditEvent(4, { at: NOW - 7 * MIN, action: "file.edit", target: "limits/limits.yml", actor: MARCO }),
  auditEvent(3, { at: NOW - 13 * MIN, action: "file.observed", target: "application.properties", via: "system", actor: null }),
  auditEvent(2, { at: NOW - 41 * MIN, action: "sync.apply", target: "ui/remote-itm-teller/teller-ui.yml", via: "system", actor: null }),
  auditEvent(1, { at: NOW - 2 * HOUR, action: "baseline.confirm", target: "v2.3.1 + 4", actor: PRIYA }),
];

type Options = {
  role: "admin" | "viewer";
  lanes: readonly SwimlaneSummary[];
  /** Replaces sitb's summary in the detail (the agent offline, a sync window). */
  sitb?: Partial<SwimlaneSummary>;
  restart?: { status: number; body: unknown };
};

function detailFor(lanes: readonly SwimlaneSummary[], options: Options, id: string): SwimlaneDetail {
  const base = lanes.find((l) => l.id === id) ?? lane(id);
  const summary = id === "sitb" ? { ...base, ...options.sitb } : base;
  return detailOf(summary, id === "sitb" ? SITB_SERVICES : [], false);
}

function build(id: string, description: string, options: Options): Scenario {
  const admin = options.role === "admin";
  const me = admin ? meBody("admin", { id: PRIYA, display_name: "Priya Shah", email: "priya.shah@example.test" }) : meBody("viewer", { id: MARCO, display_name: "Marco Diaz", email: "marco.diaz@example.test" });
  // The shell's own answers (the sidebar's approvals, the palette's searches), with the swimlane list replaced.
  const shell = shellScenarios.find((s) => s.id === (admin ? "design-shell-admin" : "design-shell-viewer"))?.responses.filter((r) => r.path !== "/api/v1/swimlanes" && r.path !== "/api/v1/me") ?? [];
  const lanes = options.lanes;
  const samples = ["sitb", "presitc", "sit"].map((swimlane) => ({ params: { id: swimlane } }));
  const responses: MockResponse[] = [
    { method: "GET", path: "/api/v1/me", body: me, headers: { "set-cookie": "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax" } },
    ...shell,
    { method: "GET", path: "/api/v1/swimlanes", body: { items: lanes, next: null } },
    { method: "GET", path: "/api/v1/swimlanes/{id}", body: (ctx: MockContext) => detailFor(lanes, options, ctx.params["id"] ?? ""), sample: samples[0] ?? {}, samples },
    {
      method: "GET",
      path: "/api/v1/swimlanes/{id}/tree",
      body: (ctx: MockContext) => ({ items: ctx.params["id"] === "sitb" ? (SITB_TREE[ctx.query.get("prefix") ?? ""] ?? []) : [], next: null }),
      sample: { params: { id: "sitb" }, query: new URLSearchParams() },
      samples: [{ params: { id: "sitb" }, query: new URLSearchParams("prefix=tx-infinity-api") }, { params: { id: "sit" }, query: new URLSearchParams() }],
    },
    {
      method: "GET",
      path: "/api/v1/swimlanes/{id}/drift",
      body: (ctx: MockContext) => ({ items: ctx.params["id"] === "sitb" ? SITB_DRIFT : [], next: null }),
      sample: { params: { id: "sitb" } },
      samples: [{ params: { id: "sitb" } }, { params: { id: "sit" } }],
    },
    {
      method: "GET",
      path: "/api/v1/findings",
      body: (ctx: MockContext): FindingPage => ({ items: FINDINGS.filter((f) => ctx.query.get("swimlane") === null || f.swimlane === ctx.query.get("swimlane")), next: null }),
      sample: { query: new URLSearchParams("swimlane=sitb") },
      samples: [{ query: new URLSearchParams() }],
    },
    ...(admin
      ? [
          {
            method: "GET" as const,
            path: "/api/v1/admin/audit",
            body: (ctx: MockContext) => ({ items: AUDIT.filter((e) => ctx.query.get("swimlane") === null || e.swimlane === ctx.query.get("swimlane")), next: null }),
            sample: { query: new URLSearchParams("swimlane=sitb") },
          },
        ]
      : []),
    { method: "POST", path: "/api/v1/swimlanes/{id}/restart", status: options.restart?.status ?? 200, body: options.restart?.body ?? { outcome: "applied", hash: null, audit_id: 7 } },
  ];
  return { id, description, responses };
}

export const scenarios: readonly Scenario[] = [
  build("design-swimlanes-six", "The Swimlanes page of the design: Priya Shah, an Admin, the six swimlanes, with the detail of sitb (services, files, drift, findings, activity).", { role: "admin", lanes: SIX }),
  build("design-swimlanes-six-viewer", "The same, for a Viewer: no Add a swimlane, no Adopt, Restart disabled, no Activity.", { role: "viewer", lanes: SIX }),
  build("design-swimlanes-forty", "The density test of the Swimlanes page: 40 swimlanes.", { role: "admin", lanes: forty() }),
  build("design-swimlanes-empty", "No swimlane is registered yet; an Admin is offered Add a swimlane.", { role: "admin", lanes: [] }),
  build("design-swimlanes-empty-viewer", "No swimlane is registered yet; a Viewer is told to ask an admin.", { role: "viewer", lanes: [] }),
  build("design-swimlanes-agent-off", "The Swimlane Detail board, agent offline: sitb's agent stopped reporting at its last scan, 14 minutes ago.", {
    role: "admin",
    lanes: SIX.map((l) => (l.id === "sitb" ? { ...l, agent: "disconnected" as const, last_scan_at: NOW - 14 * MIN } : l)),
    sitb: { agent: "disconnected", last_scan_at: NOW - 14 * MIN },
  }),
  build("design-swimlanes-syncing", "The Swimlane Detail board, sync in progress: a sync window is open on sitb.", {
    role: "admin",
    lanes: SIX,
    sitb: { syncing: { job: { name: "csp-tenant-data-sit7", uid: "run-42" }, since: NOW - 4 * MIN } },
  }),
  build("design-swimlanes-restart-approval", "Restarting a service on PRESIT is sent for approval (States A02).", { role: "admin", lanes: SIX, restart: { status: 200, body: { outcome: "proposal_created", id: 2042 } } }),
  build("design-swimlanes-restart-locked", "Restarting a service is refused: the swimlane is locked (States K01).", { role: "admin", lanes: SIX, restart: { status: 423, body: problemBody(423, "locked", "Locked", { until: null }) } }),
];
