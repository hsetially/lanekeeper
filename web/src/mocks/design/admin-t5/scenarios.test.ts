import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { collectDesignScenarios } from "../index.ts";
import { contextFor, resolveBody, type MockContext } from "../../scenario.ts";
import { scenarios } from "./index.ts";

// The design-parity scenarios of the four Admin sections that task T5 adds. src/mocks/mocks.test.ts checks every response of every
// scenario against the OpenAPI schemas, but only for the scenarios that src/mocks/design/index.ts lists; this file does the same
// check for these, so they are held to the contract whether or not they are listed yet.

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

describe("the T5 Admin scenarios", () => {
  it("are named for the folder and for the section they draw", () => {
    const ids = scenarios.map((s) => s.id);
    expect(ids.every((id) => id.startsWith("design-admin-t5-"))).toBe(true);
    for (const id of ["sentinels", "sentinels-empty", "oslogin", "oslogin-empty", "severity", "severity-empty", "retention", "retention-defaults"]) expect(ids, id).toContain(`design-admin-t5-${id}`);
  });

  it("can be collected as the slice `admin-t5`, beside the others, without a clash", () => {
    const collected = collectDesignScenarios({ "admin-t5": scenarios });
    expect(Object.keys(collected)).toHaveLength(scenarios.length);
  });

  it("describe what they draw", () => {
    for (const scenario of scenarios) expect(scenario.description.length, scenario.id).toBeGreaterThan(30);
  });

  it("sign in an Admin and answer the pending requests the navigation counts", () => {
    for (const scenario of scenarios) {
      const me = scenario.responses.find((r) => r.path === "/api/v1/me")?.body as { role?: string } | undefined;
      expect(me?.role, scenario.id).toBe("admin");
      const requests = scenario.responses.filter((r) => r.method === "GET" && r.path === "/api/v1/admin/access-requests");
      expect(requests.length, scenario.id).toBe(1);
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
