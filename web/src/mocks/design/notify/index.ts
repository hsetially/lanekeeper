// The scenarios of the notify and restart pieces (T6, slice B): the Services tab of sitb with services waiting for a notify, the
// auto-notify line, and the config-server restart that finding C11 asks for. The handoff has NO frame for any of them (plan W6,
// open question: design: handoff v2), so these scenarios are app-owned: the app is compared with its own baselines, not with the
// design. They are the Swimlanes board's scenarios (`design-swimlanes-six`, the swimlane list and the shell around it, Priya Shah
// as an Admin; or the Viewer's) with sitb's detail, the C11 finding and the three operations of the pieces replaced.
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type` for every type.
import type { ConfigServerRestartOfferResult, Finding, PickupState, RestartOffer, ServiceState, SwimlaneSummary } from "../../../api/generated/model/index.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { detailOf, finding, lane, service } from "../swimlanes/fixtures.ts";
import { scenarios as swimlaneScenarios } from "../swimlanes/index.ts";
import { CONFIG_SERVER, CONFIG_SERVER_OFFER, NO_OFFER, OFFER_SERVICES, RESTART_OFFER } from "./data.ts";
import { fileScenarios } from "./file.ts";

export { CONFIG_SERVER, CONFIG_SERVER_OFFER, NO_OFFER, OFFER_SERVICES, RESTART_OFFER };

const NOW = 1_760_000_000_000;
const MIN = 60_000;

/** sitb's services: two wait for a notify or a restart (one reports its notification setting off), one is live, one is live within its cache time. */
export const WAITING_SERVICES: readonly ServiceState[] = [
  service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 3 * MIN, pending_files: ["application.properties", "tx-infinity-api/tx-infinity-core-sit7.yml"] }),
  service("ui-service", { pickup: "needs_notify_or_restart", pending_since: NOW - 25 * MIN, pending_files: ["application.properties"], notifications_enabled: false }),
  service("document-service"),
  service("remote-itm-teller", { pickup: "live_within_ttl", live_by: NOW + 9 * MIN }),
];

/** sitb's services when a folder was created after the config-server started: the config-server and the services that ask it for their files wait for its restart. */
export const CONFIG_SERVER_SERVICES: readonly ServiceState[] = [
  service(CONFIG_SERVER, { pickup: "needs_config_server_restart", pending_since: NOW - 8 * MIN, pending_files: ["channels.yml"] }),
  service("tx-infinity-core", { pickup: "needs_config_server_restart", pending_since: NOW - 8 * MIN, pending_files: ["channels.yml"] }),
  service("ui-service"),
  service("document-service"),
];

const C11: Finding = finding(9, {
  kind: "config_server_restart_required",
  severity: "high",
  path: "channels.yml",
  message: "channels.yml changed after the config-server started.",
  locations: [],
  detected_at: NOW - 8 * MIN,
});

type Answer = { status: number; body: unknown };

type Options = {
  id: string;
  description: string;
  viewer?: boolean;
  autoNotify: boolean;
  services: readonly ServiceState[];
  /** Check C11 is open for sitb. */
  c11: boolean;
  offer: ConfigServerRestartOfferResult;
  restartOffer?: RestartOffer;
  notify?: Answer;
  restart?: Answer;
};

const key = (r: MockResponse): string => `${r.method} ${r.path}`;
const SWIMLANE = "/api/v1/swimlanes/{id}";

/** How many services are in each pickup state that is not live, as `SwimlaneDetail.pickup_counts` (only non-zero states, each once). */
function pickupCounts(services: readonly ServiceState[]): Array<[PickupState, number]> {
  const out: Array<[PickupState, number]> = [];
  for (const state of ["live_within_ttl", "needs_notify_or_restart", "needs_config_server_restart"] as const) {
    const n = services.filter((s) => s.pickup === state).length;
    if (n > 0) out.push([state, n]);
  }
  return out;
}

function build(o: Options): Scenario {
  const base = swimlaneScenarios.find((s) => s.id === (o.viewer === true ? "design-swimlanes-six-viewer" : "design-swimlanes-six"));
  const baseResponses = base?.responses ?? [];
  const lanes = ((baseResponses.find((r) => key(r) === "GET /api/v1/swimlanes")?.body as { items: SwimlaneSummary[] } | undefined)?.items ?? []).map((l) =>
    l.id === "sitb" ? { ...l, pickup_counts: pickupCounts(o.services) } : l,
  );
  const replaced = new Set([`GET /api/v1/swimlanes`, `GET ${SWIMLANE}`, "GET /api/v1/findings", `POST ${SWIMLANE}/restart`]);
  const kept = baseResponses.filter((r) => !replaced.has(key(r)));
  const kind = "config_server_restart_required";
  const responses: MockResponse[] = [
    ...kept,
    { method: "GET", path: "/api/v1/swimlanes", body: { items: lanes, next: null } },
    {
      method: "GET",
      path: SWIMLANE,
      body: (ctx: MockContext) => {
        const id = ctx.params["id"] ?? "";
        const summary = lanes.find((l) => l.id === id) ?? lane(id);
        return id === "sitb" ? detailOf(summary, o.services, o.autoNotify) : detailOf(summary, [], false);
      },
      sample: { params: { id: "sitb" } },
      samples: [{ params: { id: "sit" } }],
    },
    {
      // Only the C11 finding is listed; the sidebar's counts come from the swimlane list. The filters are the hub's: by swimlane and by kind.
      method: "GET",
      path: "/api/v1/findings",
      body: (ctx: MockContext) => {
        const swimlane = ctx.query.get("swimlane");
        const kinds = ctx.query.get("kinds")?.split(",") ?? null;
        const items = o.c11 && (swimlane === null || swimlane === "sitb") && (kinds === null || kinds.includes(kind)) ? [C11] : [];
        return { items, next: null };
      },
      sample: { query: new URLSearchParams(`swimlane=sitb&kinds=${kind}&limit=1`) },
      samples: [{ query: new URLSearchParams() }, { query: new URLSearchParams("swimlane=sit") }],
    },
    { method: "POST", path: `${SWIMLANE}/notify`, status: o.notify?.status ?? 200, body: o.notify?.body ?? { accepted: 2, audit_id: 41 } },
    { method: "GET", path: `${SWIMLANE}/config-server-restart`, headers: { "cache-control": "no-store" }, body: o.offer },
    { method: "GET", path: `${SWIMLANE}/restart-offer`, headers: { "cache-control": "no-store" }, body: o.restartOffer ?? RESTART_OFFER, sample: { params: { id: "sitb" }, query: new URLSearchParams("path=tx-infinity-api/tx-infinity-core-sit7.yml") } },
    { method: "POST", path: `${SWIMLANE}/restart`, status: o.restart?.status ?? 200, body: o.restart?.body ?? { outcome: "applied", hash: null, audit_id: 77 } },
  ];
  return { id: o.id, description: o.description, responses };
}

const locked = problemBody(423, "locked", "Locked", { until: null });

export const scenarios: readonly Scenario[] = [
  build({ id: "design-notify-services-tab", description: "The Services tab of sitb for an Admin: two services wait for a notify or a restart (one reports notifications off), auto-notify is on, and Notify services is on the tab.", autoNotify: true, services: WAITING_SERVICES, c11: false, offer: NO_OFFER }),
  build({ id: "design-notify-auto-notify-off", description: "The same, with auto-notify off.", autoNotify: false, services: WAITING_SERVICES, c11: false, offer: NO_OFFER }),
  build({ id: "design-notify-viewer", description: "The same for a Viewer: Notify services and the restart buttons are on screen and disabled, and auto-notify is read-only with no link.", viewer: true, autoNotify: true, services: WAITING_SERVICES, c11: false, offer: NO_OFFER }),
  build({ id: "design-notify-approval", description: "Notify services is sent for approval: the hub answers with a request, and nothing is sent to the config-server.", autoNotify: true, services: WAITING_SERVICES, c11: false, offer: NO_OFFER, notify: { status: 200, body: { outcome: "proposal_created", id: 2043 } } }),
  build({ id: "design-notify-partial", description: "Notify services is answered with a count lower than what was sent: the hub accepted 1 of 2 paths and does not say which.", autoNotify: true, services: WAITING_SERVICES, c11: false, offer: NO_OFFER, notify: { status: 200, body: { accepted: 1, audit_id: 42 } } }),
  build({ id: "design-notify-locked", description: "Notify services is refused: the swimlane is locked.", autoNotify: true, services: WAITING_SERVICES, c11: false, offer: NO_OFFER, notify: { status: 423, body: locked } }),
  build({ id: "design-notify-config-server", description: "Finding C11 is open on sitb and the hub offers a config-server restart: the Deployment, and four services that cannot be served while it restarts.", autoNotify: true, services: CONFIG_SERVER_SERVICES, c11: true, offer: CONFIG_SERVER_OFFER }),
  build({
    id: "design-notify-config-server-truncated",
    description: "The same offer, with a list of affected services that is not the whole of them.",
    autoNotify: true,
    services: CONFIG_SERVER_SERVICES,
    c11: true,
    offer: { offer: { service: { namespace: "sitb", name: CONFIG_SERVER }, affected: OFFER_SERVICES, truncated: true } },
  }),
  build({ id: "design-notify-config-server-none", description: "Finding C11 is open on sitb and the hub offers no restart: it lists no single config-server Deployment, and does not guess.", autoNotify: true, services: CONFIG_SERVER_SERVICES, c11: true, offer: NO_OFFER }),
  build({ id: "design-notify-config-server-approval", description: "The config-server restart is sent for approval.", autoNotify: true, services: CONFIG_SERVER_SERVICES, c11: true, offer: CONFIG_SERVER_OFFER, restart: { status: 200, body: { outcome: "proposal_created", id: 2044 } } }),
  // The file page after a write (./file.ts).
  ...fileScenarios,
];
