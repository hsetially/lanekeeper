// The scenarios of the "As served" view (T6 slice C). There is no handoff frame for it (H2), so these are not "design parity" data in the
// sense of the other folders; they are the answers the hub can give to `getServedFile`, so that the view can be built, tested and
// compared with its own baselines. The page is the swimlane's `/served` route, with a Viewer signed in (the operation is a Viewer's), a
// swimlane with two tenants, and one answer for any selection (the file, application, tenant and channel asked for are echoed back, as the
// hub does). A scenario id is `design-served-<name>`; the loader (../index.ts) picks up what this file exports.
//
// Loaded by Node (the e2e harness) as well as by Vite: relative imports with the `.ts` extension.
import type { AgentStatus, ServedFile, SwimlaneDetail } from "../../../api/generated/model/index.ts";
import { meBody } from "../../me.ts";
import { problemBody, type MockContext, type MockResponse, type Scenario } from "../../scenario.ts";
import { SHELL_LANES } from "../shell/index.ts";
import { binaryServedFile, LANE, maskedServedFile, servedFile, SERVED_PATH, TENANTS, withheldServedFile } from "./data.ts";

const csrfCookie = "__Host-lk_csrf=mockMockMockMockMockMockMockMockMockMo0; Path=/; Secure; SameSite=Lax";
const LANE_PARAMS = { params: { id: LANE } };
const ASKED = { ...LANE_PARAMS, query: new URLSearchParams({ application: "tx-infinity-core", tenant: "sit7", file: "tx-infinity-api/connection-pool.yml" }) };
const ASKED_CHANNEL = { ...LANE_PARAMS, query: new URLSearchParams({ application: "tx-infinity-core", tenant: "sit9", channel: "beta", file: "tx-infinity-api/connection-pool.yml" }) };

function lanes(agent: AgentStatus) {
  return SHELL_LANES.map((lane) => (lane.id === LANE ? { ...lane, tenants: [...TENANTS], agent } : lane));
}

function detail(agent: AgentStatus): SwimlaneDetail {
  const lane = lanes(agent).find((l) => l.id === LANE);
  return { ...(lane as NonNullable<typeof lane>), services: [], auto_notify: false };
}

/** What the request names, put into the answer: the hub echoes the application, tenant, channel and file it was asked for. */
function echo(make: () => ServedFile) {
  return ({ query }: MockContext): ServedFile => ({
    ...make(),
    application: query.get("application") ?? "tx-infinity-core",
    tenant: query.get("tenant") ?? "sit7",
    channel: query.get("channel"),
    file: query.get("file") ?? "tx-infinity-api/connection-pool.yml",
  });
}

type Options = { id: string; description: string; agent?: AgentStatus; served: Pick<MockResponse, "status" | "body"> };

function scenario({ id, description, agent = "connected", served }: Options): Scenario {
  return {
    id,
    description,
    responses: [
      { method: "GET", path: "/api/v1/me", body: meBody("viewer"), headers: { "set-cookie": csrfCookie } },
      { method: "POST", path: "/auth/logout", status: 204 },
      { method: "GET", path: "/api/v1/swimlanes", body: { items: lanes(agent), next: null } },
      { method: "GET", path: "/api/v1/swimlanes/{id}", sample: LANE_PARAMS, body: ({ params }: MockContext) => ({ ...detail(agent), id: params["id"] ?? LANE }) },
      { method: "GET", path: SERVED_PATH, sample: ASKED, samples: [ASKED_CHANNEL], ...served },
    ],
  };
}

const NOT_SERVED = { status: 404, body: problemBody(404, "not_found", "Not found") } as const;
const UNAVAILABLE = { status: 503, body: problemBody(503, "service_unavailable", "Service unavailable") } as const;

export const scenarios: readonly Scenario[] = [
  scenario({
    id: "design-served-text",
    description: "As served: a resource file as the config-server answers for tx-infinity-core and sit7, with one placeholder it left and the file the engine reads for it.",
    served: { body: echo(() => servedFile()) },
  }),
  scenario({
    id: "design-served-masked",
    description: "As served: the same file with the password masked by the hub (a marker and its mask): a hidden value, no Reveal.",
    served: { body: echo(() => maskedServedFile()) },
  }),
  scenario({
    id: "design-served-withheld",
    description: "As served: a file the hub never shows (a keystore): its hash and no text.",
    served: { body: echo(() => withheldServedFile()) },
  }),
  scenario({
    id: "design-served-not-text",
    description: "As served: an answer that is not text (a binary file): not withheld, and no text to draw.",
    served: { body: echo(() => binaryServedFile()) },
  }),
  scenario({
    id: "design-served-empty-file",
    description: "As served: the config-server served a file of 0 bytes.",
    served: { body: echo(() => servedFile({ text: "", unresolved_placeholders: [], sources: [] })) },
  }),
  scenario({
    id: "design-served-not-served",
    description: "As served: the config-server has nothing for this selection (404): a state, not a fault.",
    served: NOT_SERVED,
  }),
  scenario({
    id: "design-served-agent-offline",
    description: "As served: the swimlane's agent is offline, so the hub cannot ask the config-server (503).",
    agent: "disconnected",
    served: UNAVAILABLE,
  }),
];
