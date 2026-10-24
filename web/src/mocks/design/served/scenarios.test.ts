import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { planMasks } from "../../../components/masking/plan.ts";
import { contextFor, resolveBody, type MockResponse } from "../../scenario.ts";
import { MARKER, SERVED_PATH } from "./data.ts";
import { scenarios } from "./index.ts";

// The "As served" scenarios: the answers the hub can give to `getServedFile`. The contract check of every response is
// src/mocks/mocks.test.ts once the loader lists this folder; this file checks the same here, so that the scenarios are right before
// they are wired, and checks what they say.

type Doc = { paths: Record<string, Record<string, { responses: Record<string, { $ref?: string; content?: Record<string, { schema?: unknown }> }> }>>; components: { responses: Record<string, { content?: Record<string, { schema?: unknown }> }> } };
const doc = parse(readFileSync(new URL("../../../../../api/openapi.yaml", import.meta.url), "utf8")) as Doc;
const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
ajv.addSchema(doc, "oas");

function schemaPointer(method: string, path: string, status: number): string {
  const operation = doc.paths[path]?.[method.toLowerCase()];
  if (!operation) throw new Error(`${method} ${path} is not in api/openapi.yaml`);
  const key = operation.responses[String(status)] ? String(status) : "default";
  const entry = operation.responses[key];
  const esc = (s: string) => s.replaceAll("~", "~0").replaceAll("/", "~1");
  let base = `/paths/${esc(path)}/${method.toLowerCase()}/responses/${esc(key)}`;
  let node = entry;
  if (entry?.$ref) {
    const name = entry.$ref.split("/").pop() ?? "";
    node = doc.components.responses[name];
    base = `/components/responses/${esc(name)}`;
  }
  const media = node?.content ? Object.keys(node.content)[0] : undefined;
  return `${base}/content/${esc(media ?? "application/json")}/schema`;
}

const contexts = (response: MockResponse) => [contextFor(response), ...(response.samples ?? []).map((sample) => ({ ...contextFor(response), ...sample }))];
const responses = scenarios.flatMap((s) => s.responses.flatMap((response) => contexts(response).map((context, i) => ({ id: s.id, response, context, i }))));
const served = (id: string, query: Record<string, string> = { application: "tx-infinity-core", tenant: "sit7", file: "tx-infinity-api/connection-pool.yml" }) => {
  const response = scenarios.find((s) => s.id === id)?.responses.find((r) => r.path === SERVED_PATH) as MockResponse;
  return { response, body: resolveBody(response, { ...contextFor(response), query: new URLSearchParams(query) }) as Record<string, unknown> | undefined };
};

describe("the served scenarios", () => {
  it("are named for this slice", () => {
    expect(scenarios.map((s) => s.id)).toEqual([
      "design-served-text",
      "design-served-masked",
      "design-served-withheld",
      "design-served-not-text",
      "design-served-empty-file",
      "design-served-not-served",
      "design-served-agent-offline",
    ]);
    for (const s of scenarios) expect(s.id.startsWith("design-served-")).toBe(true);
  });

  it.each(responses.map((r) => [`${r.id}: ${r.response.method} ${r.response.path} ${r.response.status ?? 200}${r.i === 0 ? "" : ` (sample ${r.i})`}`, r] as const))("%s satisfies the contract", (_name, { response, context }) => {
    const status = response.status ?? 200;
    const operation = doc.paths[response.path]?.[response.method.toLowerCase()];
    const declared = operation?.responses[String(status)] ?? operation?.responses["default"];
    const body = resolveBody(response, context);
    if (declared?.content === undefined && declared?.$ref === undefined) {
      expect(body).toBeUndefined();
      return;
    }
    const pointer = schemaPointer(response.method, response.path, status);
    const validate = ajv.getSchema(`oas#${pointer}`) ?? ajv.compile({ $ref: `oas#${pointer}` });
    expect(validate(body), JSON.stringify(validate.errors)).toBe(true);
  });

  it("answer for any selection, and echo the one asked for, as the hub does", () => {
    const { body } = served("design-served-text", { application: "billing", tenant: "sit9", channel: "beta", file: "core/a.yml" });
    expect(body).toMatchObject({ application: "billing", tenant: "sit9", channel: "beta", file: "core/a.yml" });
    expect(served("design-served-text").body?.["channel"]).toBeNull();
  });

  it("has a masked file whose masks the plan accepts, with a marker in the text and no setting on the mask", () => {
    const { body } = served("design-served-masked");
    const masks = body?.["masks"] as { setting?: unknown }[];
    expect(masks).toHaveLength(1);
    expect(masks[0]?.setting).toBeUndefined();
    expect(body?.["text"]).toContain(MARKER);
    expect(planMasks(String(body?.["text"]), body?.["masks"] as Parameters<typeof planMasks>[1]).status).toBe("ok");
  });

  it("has a plain file with a placeholder that was left, and no masks", () => {
    const { body } = served("design-served-text");
    expect(body?.["unresolved_placeholders"]).toEqual(["AUDIT_SINK"]);
    expect(body?.["masks"]).toBeUndefined();
    expect(String(body?.["text"])).toContain("${AUDIT_SINK}");
  });

  it("has a withheld file with no text, and an answer that is not text and not withheld", () => {
    expect(served("design-served-withheld").body).toMatchObject({ content_withheld: true, text: null });
    expect(served("design-served-not-text").body).toMatchObject({ content_withheld: false, text: null });
  });

  it("has a 404 that is a state and a 503 with the agent offline", () => {
    expect(served("design-served-not-served").response.status).toBe(404);
    const offline = scenarios.find((s) => s.id === "design-served-agent-offline");
    expect(served("design-served-agent-offline").response.status).toBe(503);
    const detail = offline?.responses.find((r) => r.path === "/api/v1/swimlanes/{id}") as MockResponse;
    expect((resolveBody(detail, contextFor(detail)) as { agent: string }).agent).toBe("disconnected");
  });

  it("gives the swimlane two tenants, so the tenant is chosen and not typed", () => {
    const detail = scenarios[0]?.responses.find((r) => r.path === "/api/v1/swimlanes/{id}") as MockResponse;
    expect((resolveBody(detail, contextFor(detail)) as { tenants: string[] }).tenants).toEqual(["sit7", "sit9"]);
  });
});
