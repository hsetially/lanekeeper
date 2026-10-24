// The scenarios of the file page after a write (T6, slice B): the "Saved" banner of a file that services have not picked up yet,
// and what it offers next to the sentence "Services pick it up after a notify or a restart." The handoff has NO frame for any of
// it (plan W6), so these are app-owned, like the rest of this folder: the app is compared with its own baselines.
//
// They are the File page's own scenario (`fileScenario`, slice S3: the core tenant file for sit7 as Priya Shah, an Admin, with a
// save that is applied) with three things replaced: the file's entry says what the hub says about its pickup (`pickup_state`, which
// the tree and the head of the file read), the swimlane lists the services that wait for it, and the hub answers the three
// operations the banner's buttons start (`notifyServices`, `getRestartOffer`, `getConfigServerRestartOffer`).
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension, `import type` for every type.
import type { PickupState, ServiceState, SwimlaneDetail } from "../../../api/generated/model/index.ts";
import { problemBody, resolveBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { fileScenario } from "../file/build.ts";
import { coreFile, CORE_PATH, LANE, type FileKit } from "../file/data.ts";
import { service } from "../swimlanes/fixtures.ts";
import { CONFIG_SERVER, CONFIG_SERVER_OFFER, NO_OFFER, RESTART_OFFER } from "./data.ts";

const NOW = 1_760_000_000_000;
const MIN = 60_000;
const SWIMLANE = "/api/v1/swimlanes/{id}";
const OFFER_PATH = { params: { id: LANE }, query: new URLSearchParams({ path: CORE_PATH }) };

/** The core tenant file as the hub reports it once the save was applied: services have not picked it up. */
function waiting(pickup: PickupState): FileKit {
  const file = coreFile({ masked: false });
  return { ...file, entry: { ...file.entry, pickup_state: pickup } };
}

/** Two services read the file and wait for a notify or a restart (one reports its notification setting off); one is live. */
const NOTIFY_SERVICES: readonly ServiceState[] = [
  service("tx-infinity-core", { pickup: "needs_notify_or_restart", pending_since: NOW - 2 * MIN, pending_files: [CORE_PATH] }),
  service("ui-service", { pickup: "needs_notify_or_restart", pending_since: NOW - 2 * MIN, pending_files: [CORE_PATH], notifications_enabled: false }),
  service("document-service"),
];

/** The config-server and the services that ask it for their files wait for the config-server to restart. */
const CONFIG_SERVER_SERVICES: readonly ServiceState[] = [
  service(CONFIG_SERVER, { pickup: "needs_config_server_restart", pending_since: NOW - 2 * MIN, pending_files: [CORE_PATH] }),
  service("tx-infinity-core", { pickup: "needs_config_server_restart", pending_since: NOW - 2 * MIN, pending_files: [CORE_PATH] }),
  service("document-service"),
];

type Answer = { status: number; body: unknown };

type Options = {
  id: string;
  description: string;
  pickup: PickupState;
  services: readonly ServiceState[];
  notify?: Answer;
  restart?: Answer;
  /** What `getConfigServerRestartOffer` answers. */
  offer?: typeof CONFIG_SERVER_OFFER;
};

const key = (r: MockResponse): string => `${r.method} ${r.path}`;

function build(o: Options): Scenario {
  const base = fileScenario({ id: o.id, description: o.description, file: waiting(o.pickup) });
  // The pickup counts of the swimlane, as the list and the detail say them (only the states that are not live).
  const counts = (["needs_notify_or_restart", "needs_config_server_restart"] as const)
    .map((state) => [state, o.services.filter((s) => s.pickup === state).length] as const)
    .filter(([, n]) => n > 0)
    .map(([state, n]) => [state, n] as [PickupState, number]);
  const replaced = new Set([`GET ${SWIMLANE}`, `POST ${SWIMLANE}/restart`]);
  const detail = base.responses.find((r) => key(r) === `GET ${SWIMLANE}`);
  if (detail === undefined) throw new Error("the file scenario has no swimlane detail");
  const kept = base.responses.filter((r) => !replaced.has(key(r)));
  const responses: MockResponse[] = [
    ...kept,
    {
      // The File page's own detail of the swimlane (its tier, tenants and agent), with the services that wait and how many are in each state.
      method: "GET",
      path: SWIMLANE,
      sample: { params: { id: LANE } },
      body: (ctx: MockContext): SwimlaneDetail => ({ ...(resolveBody(detail, ctx) as SwimlaneDetail), services: [...o.services], pickup_counts: counts }),
    },
    { method: "POST", path: `${SWIMLANE}/notify`, status: o.notify?.status ?? 200, body: o.notify?.body ?? { accepted: 1, audit_id: 41 } },
    { method: "GET", path: `${SWIMLANE}/restart-offer`, headers: { "cache-control": "no-store" }, body: RESTART_OFFER, sample: OFFER_PATH },
    { method: "GET", path: `${SWIMLANE}/config-server-restart`, headers: { "cache-control": "no-store" }, body: o.offer ?? NO_OFFER },
    { method: "POST", path: `${SWIMLANE}/restart`, status: o.restart?.status ?? 200, body: o.restart?.body ?? { outcome: "applied", hash: null, audit_id: 77 } },
  ];
  return { id: o.id, description: o.description, responses };
}

const locked = problemBody(423, "locked", "Locked", { until: null });

export const fileScenarios: readonly Scenario[] = [
  build({
    id: "design-notify-file-saved",
    description: "File, after a save: two services read the file and wait for a notify or a restart. The Saved banner offers Notify services (this file) and Restart services (the hub's restart-all offer).",
    pickup: "needs_notify_or_restart",
    services: NOTIFY_SERVICES,
  }),
  build({
    id: "design-notify-file-locked",
    description: "The same, and Notify services is refused: the swimlane is locked.",
    pickup: "needs_notify_or_restart",
    services: NOTIFY_SERVICES,
    notify: { status: 423, body: locked },
  }),
  build({
    id: "design-notify-file-live",
    description: "File, after a save, once the services have picked the file up: the Saved banner says only that the file is the new version, and offers nothing.",
    pickup: "live",
    services: [service("tx-infinity-core"), service("ui-service"), service("document-service")],
  }),
  build({
    id: "design-notify-file-config-server",
    description: "File, after a save: the file is one that the config-server only serves after it restarts. The Saved banner offers Restart config-server, and the hub has an offer for it.",
    pickup: "needs_config_server_restart",
    services: CONFIG_SERVER_SERVICES,
    offer: CONFIG_SERVER_OFFER,
  }),
];
