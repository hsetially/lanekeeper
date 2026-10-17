import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { getResponse } from "msw";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { DEFAULT_SCENARIO, scenarios } from "./scenarios.ts";
import { contextFor, mswPath, resolveBody, toHandlers, type MockContext, type MockResponse } from "./scenario.ts";

// Every mock response must satisfy the OpenAPI schema of its operation and status (patterns and lengths included), so a
// mock cannot drift from the contract and a screen is never built against an answer the server could not give.

type Doc = {
  paths: Record<string, Record<string, { operationId: string; responses: Record<string, { $ref?: string; content?: Record<string, { schema?: unknown }> }> }>>;
  components: { responses: Record<string, { content?: Record<string, { schema?: unknown }> }> };
};
const doc = parse(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as Doc;

const ajv = new Ajv2020({ strict: false, allErrors: true, validateFormats: false });
ajv.addSchema(doc, "oas");

/** The JSON pointer of the schema of an operation's response, following one `$ref` of a shared response. */
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

// A response is checked once with its `sample`, and once more for each of its `samples`: a list of pages (first, middle, last).
const all: Array<{ scenario: string; response: MockResponse; sample?: Partial<MockContext>; label: string }> = Object.values(scenarios).flatMap((s) =>
  s.responses.flatMap((response) => [{ scenario: s.id, response, label: "" }, ...(response.samples ?? []).map((sample, i) => ({ scenario: s.id, response, sample, label: ` (sample ${i + 1})` }))]),
);

describe("scenarios", () => {
  it("has the sessions T1 needs and the data of the performance runs, and a default that exists", () => {
    expect(Object.keys(scenarios)).toEqual(expect.arrayContaining(["admin", "disabled", "editor", "needs-token", "operator", "pending", "signed-out", "viewer", "perf", "perf-full", "perf-viewer", "live"]));
    expect(scenarios[DEFAULT_SCENARIO]).toBeDefined();
  });

  it("has an id that is its key", () => {
    for (const [key, scenario] of Object.entries(scenarios)) expect(scenario.id).toBe(key);
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
    const ok = validate(body);
    expect(ok, JSON.stringify(validate.errors)).toBe(true);
  });

  it("refuses a body that breaks the contract (the validator can fail)", () => {
    const pointer = pointerFor("GET", "/api/v1/me", 200);
    const validate = ajv.compile({ $ref: `oas#${pointer}` });
    expect(validate({ id: "not-an-id" })).toBe(false);
    const good = resolveBody(scenarios["viewer"]?.responses[0] as MockResponse, contextFor(scenarios["viewer"]?.responses[0] as MockResponse)) as Record<string, unknown>;
    expect(validate({ ...good, display_name: "x".repeat(10_000) })).toBe(false);
    expect(validate({ ...good, role: "superuser" })).toBe(false);
    expect(validate({ ...good, extra: 1 })).toBe(false);
  });
});

describe("toHandlers", () => {
  it("turns an OpenAPI template into msw's path syntax", () => {
    expect(mswPath("/api/v1/swimlanes/{id}/files")).toBe("*/api/v1/swimlanes/:id/files");
  });

  it("serves what the scenario says, with the content type of the status", async () => {
    const handlers = toHandlers(scenarios["admin"] as never);
    const ok = await getResponse(handlers, new Request("http://localhost/api/v1/me"));
    expect(ok?.status).toBe(200);
    expect(ok?.headers.get("content-type")).toBe("application/json");
    expect(((await ok?.json()) as { role: string }).role).toBe("admin");
    const out = toHandlers(scenarios["signed-out"] as never);
    const denied = await getResponse(out, new Request("http://localhost/api/v1/me"));
    expect(denied?.status).toBe(401);
    expect(denied?.headers.get("content-type")).toBe("application/problem+json");
    const none = await getResponse(out, new Request("http://localhost/auth/logout", { method: "POST" }));
    expect(none?.status).toBe(204);
  });

  it("answers 304 with no body when a response has an ETag and the request has it in If-None-Match, as the hub does", async () => {
    const handlers = toHandlers({
      id: "etag",
      description: "t",
      responses: [{ method: "GET", path: "/api/v1/compare", headers: { etag: '"abc"' }, body: { hunks: [] } }],
    });
    const first = await getResponse(handlers, new Request("http://localhost/api/v1/compare"));
    expect(first?.status).toBe(200);
    expect(first?.headers.get("etag")).toBe('"abc"');
    const same = await getResponse(handlers, new Request("http://localhost/api/v1/compare", { headers: { "if-none-match": '"abc"' } }));
    expect(same?.status).toBe(304);
    expect(same?.headers.get("etag")).toBe('"abc"');
    expect(await same?.text()).toBe("");
    const other = await getResponse(handlers, new Request("http://localhost/api/v1/compare", { headers: { "if-none-match": '"zzz"' } }));
    expect(other?.status).toBe(200);
  });

  it("does not make a 304 of a response that has no ETag, or of an error", async () => {
    const handlers = toHandlers({ id: "t", description: "t", responses: [{ method: "GET", path: "/api/v1/me", body: { a: 1 } }] });
    const r = await getResponse(handlers, new Request("http://localhost/api/v1/me", { headers: { "if-none-match": '"abc"' } }));
    expect(r?.status).toBe(200);
  });

  it("does not answer what the scenario does not know", async () => {
    expect(await getResponse(toHandlers(scenarios["viewer"] as never), new Request("http://localhost/api/v1/swimlanes"))).toBeUndefined();
  });
});
