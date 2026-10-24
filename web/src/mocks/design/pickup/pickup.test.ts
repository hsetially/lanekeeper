import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { collectDesignScenarios } from "../index.ts";
import { contextFor, resolveBody, type MockContext } from "../../scenario.ts";
import { scenarios } from "./index.ts";

// The scenarios of the pickup chips. src/mocks/mocks.test.ts checks every response of every scenario against the OpenAPI schemas,
// but only for the scenarios that src/mocks/design/index.ts lists; this file does the same check for these, so they are held to
// the contract whether or not they are listed yet.

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

const all = scenarios.flatMap((s) =>
  s.responses.flatMap((response) => [{ scenario: s.id, response, label: "", sample: undefined as Partial<MockContext> | undefined }, ...(response.samples ?? []).map((sample, i) => ({ scenario: s.id, response, label: ` (sample ${i + 1})`, sample }))]),
);

describe("the pickup scenarios", () => {
  it("are named for the folder", () => {
    expect(scenarios.map((s) => s.id).sort()).toEqual(["design-pickup-hostile", "design-pickup-quiet", "design-pickup-states", "design-pickup-unstated"]);
  });

  it("can be collected as the slice `pickup`, beside the others, without a clash", () => {
    expect(Object.keys(collectDesignScenarios({ pickup: scenarios }))).toHaveLength(scenarios.length);
  });

  it("describe what they draw", () => {
    for (const scenario of scenarios) expect(scenario.description.length, scenario.id).toBeGreaterThan(40);
  });

  it("put the services in all four states in the first, and the same counts in the Pickup column that the services add up to", () => {
    const states = scenarios.find((s) => s.id === "design-pickup-states");
    const detail = states?.responses.find((r) => r.path === "/api/v1/swimlanes/{id}");
    const sitb = resolveBody(detail ?? { method: "GET", path: "" }, { ...contextFor(detail ?? { method: "GET", path: "" }), params: { id: "sitb" } }) as { services: Array<{ pickup: string }>; pickup_counts: Array<[string, number]> };
    for (const state of ["live", "live_within_ttl", "needs_notify_or_restart", "needs_config_server_restart"]) expect(sitb.services.some((s) => s.pickup === state), state).toBe(true);
    for (const [state, n] of sitb.pickup_counts) expect(sitb.services.filter((s) => s.pickup === state).length, state).toBe(n);
  });

  it("state no pickup for any file in the one that draws the hub as it is", () => {
    const unstated = scenarios.find((s) => s.id === "design-pickup-unstated");
    const tree = unstated?.responses.find((r) => r.path === "/api/v1/swimlanes/{id}/tree");
    for (const prefix of ["", "tx-infinity-api", "new-folder"]) {
      const body = resolveBody(tree ?? { method: "GET", path: "" }, { ...contextFor(tree ?? { method: "GET", path: "" }), params: { id: "sitb" }, query: new URLSearchParams(prefix === "" ? "" : `prefix=${prefix}`) }) as { items: Array<{ pickup_state: string | null }> };
      expect(body.items.length, prefix).toBeGreaterThan(0);
      for (const entry of body.items) expect(entry.pickup_state).toBeNull();
    }
  });

  it.each(all.map((a) => [`${a.scenario}: ${a.response.method} ${a.response.path} ${a.response.status ?? 200}${a.label}`, a] as const))("%s satisfies the contract", (_name, { response, sample }) => {
    const status = response.status ?? 200;
    const pointer = pointerFor(response.method, response.path, status);
    const body = resolveBody(response, sample ? { ...contextFor(response), ...sample } : contextFor(response));
    if (pointer === undefined) {
      expect(body, "an operation with no body in the contract sends none").toBeUndefined();
      return;
    }
    const validate = ajv.getSchema(`oas#${pointer}`) ?? ajv.compile({ $ref: `oas#${pointer}` });
    expect(validate(body), JSON.stringify(validate.errors)).toBe(true);
  });
});
