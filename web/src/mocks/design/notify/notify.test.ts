import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import type { FileEntry, Finding, SwimlaneDetail } from "../../../api/generated/model/index.ts";
import { contextFor, resolveBody, type MockContext, type MockResponse } from "../../scenario";
import { CONFIG_SERVER, CONFIG_SERVER_OFFER, scenarios } from "./index";

// The scenarios of the notify and restart pieces. `src/mocks/mocks.test.ts` checks every response of every registered scenario
// against the OpenAPI schema; the same check runs here as well, with the samples of each response, and the rest checks that the data
// says what the screens need it to say.

type Doc = {
  paths: Record<string, Record<string, { operationId: string; responses: Record<string, { $ref?: string; content?: Record<string, { schema?: unknown }> }> }>>;
  components: { responses: Record<string, { content?: Record<string, { schema?: unknown }> }> };
};
const doc = parse(readFileSync(new URL("../../../../../api/openapi.yaml", import.meta.url), "utf8")) as Doc;
const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
ajv.addSchema(doc, "oas");

function pointerFor(method: string, path: string, status: number): string | undefined {
  const operation = doc.paths[path]?.[method.toLowerCase()];
  if (!operation) throw new Error(`${method} ${path} is not in api/openapi.yaml`);
  const entry = operation.responses[String(status)] ?? operation.responses["default"];
  if (!entry) throw new Error(`${operation.operationId} declares no ${status} response`);
  const esc = (s: string) => s.replaceAll("~", "~0").replaceAll("/", "~1");
  let base = `/paths/${esc(path)}/${method.toLowerCase()}/responses/${esc(operation.responses[String(status)] ? String(status) : "default")}`;
  let node: { content?: Record<string, { schema?: unknown }> } = entry;
  if (entry.$ref) {
    const name = entry.$ref.split("/").pop() ?? "";
    node = doc.components.responses[name] ?? {};
    base = `/components/responses/${esc(name)}`;
  }
  const media = node.content ? Object.keys(node.content)[0] : undefined;
  return media && node.content?.[media]?.schema ? `${base}/content/${esc(media)}/schema` : undefined;
}

const all = scenarios.flatMap((s) => s.responses.flatMap((response) => [{ scenario: s.id, response, sample: undefined as Partial<MockContext> | undefined, label: "" }, ...(response.samples ?? []).map((sample, i) => ({ scenario: s.id, response, sample, label: ` (sample ${i + 1})` }))]));

describe("the ids", () => {
  it("are design-notify-<name>, each once", () => {
    expect(scenarios.length).toBeGreaterThanOrEqual(9);
    for (const s of scenarios) expect(s.id).toMatch(/^design-notify-[a-z-]+$/);
    expect(new Set(scenarios.map((s) => s.id)).size).toBe(scenarios.length);
  });
});

describe("every response satisfies the contract", () => {
  it.each(all.map((a) => [`${a.scenario}: ${a.response.method} ${a.response.path} ${a.response.status ?? 200}${a.label}`, a] as const))("%s", (_name, { response, sample }) => {
    const status = response.status ?? 200;
    const pointer = pointerFor(response.method, response.path, status);
    const body = resolveBody(response, sample ? { ...contextFor(response), ...sample } : contextFor(response));
    if (pointer === undefined) {
      expect(body).toBeUndefined();
      return;
    }
    const validate = ajv.getSchema(`oas#${pointer}`) ?? ajv.compile({ $ref: `oas#${pointer}` });
    expect(validate(body), JSON.stringify(validate.errors)).toBe(true);
  });
});

const byId = (id: string) => {
  const found = scenarios.find((s) => s.id === id);
  if (!found) throw new Error(`no scenario ${id}`);
  return found;
};
const find = (id: string, method: string, path: string): MockResponse => {
  const found = byId(id).responses.find((r) => r.method === method && r.path === path);
  if (!found) throw new Error(`${id} has no ${method} ${path}`);
  return found;
};
const detail = (id: string, lane = "sitb"): SwimlaneDetail => {
  const response = find(id, "GET", "/api/v1/swimlanes/{id}");
  return resolveBody(response, { ...contextFor(response), params: { id: lane } }) as SwimlaneDetail;
};
const findings = (id: string, query: string): Finding[] => {
  const response = find(id, "GET", "/api/v1/findings");
  return (resolveBody(response, { ...contextFor(response), query: new URLSearchParams(query) }) as { items: Finding[] }).items;
};

describe("the Services tab", () => {
  it("has services waiting for a notify, one live and one live within its cache time, and the counts agree with them", () => {
    const sitb = detail("design-notify-services-tab");
    expect(sitb.services.map((s) => s.pickup)).toEqual(["needs_notify_or_restart", "needs_notify_or_restart", "live", "live_within_ttl"]);
    expect(sitb.pickup_counts).toEqual([["live_within_ttl", 1], ["needs_notify_or_restart", 2]]);
    expect(sitb.services.find((s) => s.service.name === "ui-service")?.notifications_enabled).toBe(false);
    expect(sitb.auto_notify).toBe(true);
  });

  it("has auto-notify off in its own scenario, and the other swimlanes without services", () => {
    expect(detail("design-notify-auto-notify-off").auto_notify).toBe(false);
    expect(detail("design-notify-services-tab", "sit").services).toEqual([]);
  });

  it("is a Viewer's, with the same services", () => {
    const viewer = find("design-notify-viewer", "GET", "/api/v1/me");
    expect((resolveBody(viewer, contextFor(viewer)) as { role: string }).role).toBe("viewer");
    expect(detail("design-notify-viewer").services).toHaveLength(4);
  });

  it("answers a notify with a count, a request for approval, a count lower than what was sent, or a lock", () => {
    const answer = (id: string) => find(id, "POST", "/api/v1/swimlanes/{id}/notify");
    expect(answer("design-notify-services-tab").body).toEqual({ accepted: 2, audit_id: 41 });
    expect(answer("design-notify-approval").body).toEqual({ outcome: "proposal_created", id: 2043 });
    expect(answer("design-notify-partial").body).toEqual({ accepted: 1, audit_id: 42 });
    expect(answer("design-notify-locked").status).toBe(423);
  });
});

describe("the config-server restart", () => {
  it("lists the C11 finding for sitb, and only for the kind and swimlane asked", () => {
    const query = "swimlane=sitb&kinds=config_server_restart_required&limit=1";
    expect(findings("design-notify-config-server", query).map((f) => f.kind)).toEqual(["config_server_restart_required"]);
    expect(findings("design-notify-config-server", "swimlane=sit&kinds=config_server_restart_required")).toEqual([]);
    expect(findings("design-notify-config-server", "swimlane=sitb&kinds=duplicate_keys")).toEqual([]);
    expect(findings("design-notify-services-tab", query)).toEqual([]);
  });

  it("has services that wait for the config-server to restart, among them the Deployment the offer names", () => {
    const sitb = detail("design-notify-config-server");
    expect(sitb.services.filter((s) => s.pickup === "needs_config_server_restart").map((s) => s.service.name)).toContain(CONFIG_SERVER);
    expect(sitb.pickup_counts).toEqual([["needs_config_server_restart", 2]]);
  });

  it("offers the Deployment and the services it affects, a list that is not the whole of them, or nothing", () => {
    const offer = (id: string) => find(id, "GET", "/api/v1/swimlanes/{id}/config-server-restart").body as typeof CONFIG_SERVER_OFFER;
    expect(offer("design-notify-config-server").offer?.service.name).toBe(CONFIG_SERVER);
    expect(offer("design-notify-config-server").offer?.truncated).toBe(false);
    expect(offer("design-notify-config-server-truncated").offer?.truncated).toBe(true);
    expect(offer("design-notify-config-server-none").offer).toBeNull();
    expect(offer("design-notify-services-tab").offer).toBeNull();
  });

  it("is answered `no-store`, as the hub does", () => {
    expect(find("design-notify-config-server", "GET", "/api/v1/swimlanes/{id}/config-server-restart").headers).toEqual({ "cache-control": "no-store" });
    expect(find("design-notify-config-server", "GET", "/api/v1/swimlanes/{id}/restart-offer").headers).toEqual({ "cache-control": "no-store" });
  });

  it("can be sent for approval", () => {
    expect(find("design-notify-config-server-approval", "POST", "/api/v1/swimlanes/{id}/restart").body).toEqual({ outcome: "proposal_created", id: 2044 });
  });
});

describe("the file page after a write", () => {
  const CORE = "tx-infinity-api/tx-infinity-core-sit7.yml";
  const entryOf = (id: string): FileEntry | undefined => {
    const response = find(id, "GET", "/api/v1/swimlanes/{id}/tree");
    const tree = resolveBody(response, { ...contextFor(response), query: new URLSearchParams({ prefix: "tx-infinity-api" }) }) as { items: FileEntry[] };
    return tree.items.find((e) => e.path === CORE);
  };

  it("has the file in the pickup state the banner is made for, as the tree sends it", () => {
    expect(entryOf("design-notify-file-saved")?.pickup_state).toBe("needs_notify_or_restart");
    expect(entryOf("design-notify-file-locked")?.pickup_state).toBe("needs_notify_or_restart");
    expect(entryOf("design-notify-file-live")?.pickup_state).toBe("live");
    expect(entryOf("design-notify-file-config-server")?.pickup_state).toBe("needs_config_server_restart");
  });

  it("lists the services that wait on the file, and the counts agree with them", () => {
    const saved = detail("design-notify-file-saved");
    expect(saved.services.filter((s) => s.pickup === "needs_notify_or_restart").map((s) => s.service.name)).toEqual(["tx-infinity-core", "ui-service"]);
    expect(saved.services.every((s) => s.pickup !== "needs_notify_or_restart" || s.pending_files.includes(CORE))).toBe(true);
    expect(saved.pickup_counts).toEqual([["needs_notify_or_restart", 2]]);
    const configServer = detail("design-notify-file-config-server");
    expect(configServer.services.filter((s) => s.pickup === "needs_config_server_restart").map((s) => s.service.name)).toContain(CONFIG_SERVER);
    expect(configServer.pickup_counts).toEqual([["needs_config_server_restart", 2]]);
  });

  it("keeps the File page's own swimlane (its agent, tier and tenants), with the same id", () => {
    expect(detail("design-notify-file-saved").id).toBe("sitb");
    expect(detail("design-notify-file-saved").tenants).toContain("sit7");
    expect(detail("design-notify-file-saved").agent).toBe("connected");
  });

  it("answers the three operations the banner's buttons start, and a save that is applied", () => {
    expect(find("design-notify-file-saved", "POST", "/api/v1/swimlanes/{id}/notify").body).toEqual({ accepted: 1, audit_id: 41 });
    expect(find("design-notify-file-locked", "POST", "/api/v1/swimlanes/{id}/notify").status).toBe(423);
    expect((find("design-notify-file-saved", "GET", "/api/v1/swimlanes/{id}/restart-offer").body as { services: unknown[] }).services).toHaveLength(2);
    expect((find("design-notify-file-config-server", "GET", "/api/v1/swimlanes/{id}/config-server-restart").body as typeof CONFIG_SERVER_OFFER).offer?.service.name).toBe(CONFIG_SERVER);
    expect(find("design-notify-file-saved", "PUT", "/api/v1/swimlanes/{id}/files").body).toMatchObject({ outcome: "applied" });
    expect(find("design-notify-file-saved", "GET", "/api/v1/swimlanes/{id}/restart-offer").headers).toEqual({ "cache-control": "no-store" });
  });
});
