// The scenarios of the pickup chips (T6, slice A): the Services tab, the swimlane list's Pickup column, the head of a swimlane, the
// file tree and the head of a file, with services and files in each of the four states of D85. The handoff has no frame for any of
// them (plan F8, W6), so these are app-owned baselines and the data is what the contract can carry, drawn on the Swimlane Detail
// board's swimlane (sitb) so that everything else on the screen is the design's. The signed-in person is Priya Shah, an Admin.
//
// Times are offsets from the frozen clock the visual runs use (1_760_000_000_000, 2025-10-09 08:53:20 UTC), so "Live by 09:15 UTC"
// reads the same every time and a time before it is a time that has passed.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type`.
import type { FileEntry, PickupState, ServiceState, SwimlaneDetail, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import type { MockContext, MockResponse, Scenario } from "../../scenario.ts";
import { detailOf, fileEntry, folderEntry, service } from "../swimlanes/fixtures.ts";
import { scenarios as swimlaneScenarios } from "../swimlanes/index.ts";

const NOW = 1_760_000_000_000;
const MIN = 60_000;
/** 09:15:00 UTC on the day of the frozen clock: 22 minutes to come. */
const LIVE_BY_FUTURE = Date.UTC(2025, 9, 9, 9, 15);
/** 08:40:00 UTC on the day of the frozen clock: 13 minutes gone. */
const LIVE_BY_PAST = Date.UTC(2025, 9, 9, 8, 40);

const SERVICES: readonly ServiceState[] = [
  service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 12 * MIN, notifications_enabled: true, pending_files: ["tx-infinity-api/tx-infinity-core-sit7.yml", "application.properties"] }),
  service("ui-service", { pickup: "needs_notify_or_restart", pending_since: NOW - 25 * MIN, notifications_enabled: false, pending_files: ["application.properties"] }),
  service("config-server", { pickup: "needs_config_server_restart", pending_since: NOW - MIN, notifications_enabled: true, pending_files: ["channels.yml", "new-folder/new-settings.yml"] }),
  service("remote-itm-teller", { pickup: "live_within_ttl", pending_since: NOW - 8 * MIN, live_by: LIVE_BY_FUTURE, cache_ttl_secs: 1200, pending_files: ["ui/remote-itm-teller/teller-labels.json"] }),
  service("document-service", { pickup: "live_within_ttl", pending_since: NOW - 33 * MIN, live_by: LIVE_BY_PAST, cache_ttl_secs: 1200, pending_files: ["document-service/resources/receipt-logo.jpeg"] }),
  service("holds-service", { pickup: "live_within_ttl", pending_since: NOW - 2 * MIN, live_by: null, cache_ttl_secs: 300 }),
  service("limits-service"),
];

const COUNTS: SwimlaneSummary["pickup_counts"] = [
  ["needs_config_server_restart", 1],
  ["needs_notify_or_restart", 2],
  ["live_within_ttl", 3],
];

/** The tree of sitb with the pickup state of each file as the hub would state it, or with none (the hub does not fill it in yet). */
function tree(stated: boolean): Record<string, FileEntry[]> {
  const state = (s: PickupState): { pickup_state: PickupState | null } => ({ pickup_state: stated ? s : null });
  return {
    "": [
      folderEntry("tx-infinity-api/"),
      folderEntry("ui/remote-itm-teller/"),
      folderEntry("new-folder/", state("needs_config_server_restart")),
      folderEntry("limits/"),
      fileEntry("application.properties", { drift: "nfs_ahead", logical_file: "application.properties", ...state("needs_notify_or_restart") }),
      fileEntry("channels.yml", { logical_file: "channels.yml", ...state("needs_config_server_restart") }),
    ],
    "tx-infinity-api": [
      fileEntry("tx-infinity-api/tx-infinity-core.yml", { logical_file: "tx-infinity-api/tx-infinity-core.yml", ...state("live") }),
      fileEntry("tx-infinity-api/tx-infinity-core-sit7.yml", { kind: "tenant", tenant: "sit7", logical_file: "tx-infinity-api/tx-infinity-core.yml", drift: "nfs_ahead", ...state("needs_notify_or_restart") }),
      fileEntry("tx-infinity-api/tx-infinity-gateway.yml", { logical_file: "tx-infinity-api/tx-infinity-gateway.yml", ...state("live") }),
    ],
    "ui/remote-itm-teller": [
      fileEntry("ui/remote-itm-teller/teller-ui.yml", { ...state("live") }),
      fileEntry("ui/remote-itm-teller/teller-labels.json", { role: "resource", ...state("live_within_ttl") }),
    ],
    "new-folder": [fileEntry("new-folder/new-settings.yml", { ...state("needs_config_server_restart") })],
    limits: [fileEntry("limits/limits.yml", { ...state("live") }), fileEntry("limits/channel-limits.yml", { ...state("live") })],
  };
}

type Options = {
  services: readonly ServiceState[];
  counts: SwimlaneSummary["pickup_counts"];
  tree: Record<string, FileEntry[]>;
};

/** What the Swimlane Detail board's data already answers, without the three answers this folder replaces. */
function around(): readonly MockResponse[] {
  const base = swimlaneScenarios.find((s) => s.id === "design-swimlanes-six")?.responses ?? [];
  return base.filter((r) => !(r.method === "GET" && (r.path === "/api/v1/swimlanes" || r.path === "/api/v1/swimlanes/{id}" || r.path === "/api/v1/swimlanes/{id}/tree")));
}

function build(id: string, description: string, options: Options): Scenario {
  const six = swimlaneScenarios.find((s) => s.id === "design-swimlanes-six")?.responses.find((r) => r.method === "GET" && r.path === "/api/v1/swimlanes")?.body as { items: SwimlaneSummary[] } | undefined;
  const lanes = (six?.items ?? []).map((l): SwimlaneSummary => (l.id === "sitb" ? { ...l, ...(options.counts === undefined ? {} : { pickup_counts: options.counts }) } : l));
  const withoutCounts = lanes.map((l): SwimlaneSummary => {
    if (l.id !== "sitb" || options.counts !== undefined) return l;
    const { pickup_counts: _dropped, ...rest } = l;
    return rest;
  });
  const detail = (swimlane: string): SwimlaneDetail => {
    const summary = withoutCounts.find((l) => l.id === swimlane) ?? withoutCounts[0];
    if (summary === undefined) throw new Error("design-swimlanes-six has no swimlane");
    return detailOf(summary, swimlane === "sitb" ? options.services : [], false);
  };
  const samples = ["sitb", "sit"].map((swimlane) => ({ params: { id: swimlane } }));
  const responses: MockResponse[] = [
    ...around(),
    { method: "GET", path: "/api/v1/swimlanes", body: { items: withoutCounts, next: null } },
    { method: "GET", path: "/api/v1/swimlanes/{id}", body: (ctx: MockContext) => detail(ctx.params["id"] ?? "sitb"), sample: samples[0] ?? {}, samples },
    {
      method: "GET",
      path: "/api/v1/swimlanes/{id}/tree",
      body: (ctx: MockContext) => ({ items: ctx.params["id"] === "sitb" ? (options.tree[ctx.query.get("prefix") ?? ""] ?? []) : [], next: null }),
      sample: { params: { id: "sitb" }, query: new URLSearchParams() },
      samples: [{ params: { id: "sitb" }, query: new URLSearchParams("prefix=tx-infinity-api") }, { params: { id: "sit" }, query: new URLSearchParams() }],
    },
    {
      method: "GET",
      path: "/api/v1/swimlanes/{id}/pending-restarts",
      body: (ctx: MockContext) => ({ items: ctx.params["id"] === "sitb" ? options.services.filter((s) => s.pickup !== "live") : [], next: null }),
      sample: { params: { id: "sitb" } },
      samples: [{ params: { id: "sitb" } }, { params: { id: "sit" } }],
    },
  ];
  return { id, description, responses };
}

/** Names and paths a hostile share can hold: a right-to-left override, a zero-width space, markup. */
const HOSTILE_FILES = ["tx-infinity-api/inv‮oice.yml", "tx-infinity-api/<img src=x onerror=alert(1)>.yml", "tx-infinity-api/ze​ro.yml"];

export const scenarios: readonly Scenario[] = [
  build("design-pickup-states", "The Swimlane Detail board's sitb with a service or a file in every state of D85: Needs notify or restart (notifications on and off), Needs config-server restart, Live by 09:15 UTC (client cache), a time that has passed, no time, and Live; the Pickup column counts one, two and three.", {
    services: SERVICES,
    counts: COUNTS,
    tree: tree(true),
  }),
  build("design-pickup-unstated", "The hub as it is today: the services say what they wait on, and no file or folder carries a pickup state (the registry fills FileEntry.pickup_state with null), so the tree and the head of a file draw no pickup chip.", {
    services: SERVICES,
    counts: COUNTS,
    tree: tree(false),
  }),
  build("design-pickup-quiet", "Nothing is waiting on sitb: every service is Live, the Pickup column says 0 and the hub sends no pickup_counts.", {
    services: [service("tx-infinity-core"), service("ui-service"), service("limits-service")],
    counts: undefined,
    tree: tree(false),
  }),
  build("design-pickup-hostile", "Text the page must not trust: files a service waits on whose names hold a right-to-left override, markup and a zero-width space, a live_by that is not a time of day (after the year 9999 and before 1970), and a tree of the same names.", {
    services: [
      service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 3 * MIN, pending_files: HOSTILE_FILES }),
      service("ui-service", { pickup: "live_within_ttl", live_by: 9_000_000_000_000_000, cache_ttl_secs: 300 }),
      service("holds-service", { pickup: "live_within_ttl", live_by: -86_400_000, cache_ttl_secs: 300 }),
    ],
    counts: [
      ["needs_notify_or_restart", 1],
      ["live_within_ttl", 2],
    ],
    tree: {
      "": [folderEntry("tx-infinity-api/")],
      "tx-infinity-api": HOSTILE_FILES.map((path) => fileEntry(path, { pickup_state: "needs_notify_or_restart" })),
    },
  }),
];
