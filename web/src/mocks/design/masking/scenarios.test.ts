import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { parse } from "yaml";
import { describe, expect, it } from "vitest";
import { contextFor, resolveBody, type MockResponse } from "../../scenario.ts";
import { scenarios } from "./index.ts";
import { multiMaskFile, REVEAL_PATH, REVEALED_VALUE, SECRET_SETTING } from "./data.ts";
import { planMasks } from "../../../components/masking/plan.ts";

// The masking scenarios: the File page with a masked value, and what `revealValue` answers. The contract check of every response is
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

const responses = scenarios.flatMap((s) => s.responses.map((response) => ({ id: s.id, response })));

describe("the masking scenarios", () => {
  it("are named for this slice", () => {
    expect(scenarios.map((s) => s.id)).toEqual(["design-masking-reveal", "design-masking-viewer", "design-masking-throttled", "design-masking-unavailable", "design-masking-gone", "design-masking-multi"]);
    for (const s of scenarios) expect(s.id.startsWith("design-masking-")).toBe(true);
  });

  it.each(responses.map((r) => [`${r.id}: ${r.response.method} ${r.response.path} ${r.response.status ?? 200}`, r.response] as const))("%s satisfies the contract", (_name, response: MockResponse) => {
    const status = response.status ?? 200;
    const operation = doc.paths[response.path]?.[response.method.toLowerCase()];
    const declared = operation?.responses[String(status)] ?? operation?.responses["default"];
    const body = resolveBody(response, contextFor(response));
    if (declared?.content === undefined && declared?.$ref === undefined) {
      expect(body).toBeUndefined();
      return;
    }
    const pointer = schemaPointer(response.method, response.path, status);
    const validate = ajv.getSchema(`oas#${pointer}`) ?? ajv.compile({ $ref: `oas#${pointer}` });
    expect(validate(body), JSON.stringify(validate.errors)).toBe(true);
  });

  it("answer a reveal in every scenario, never from a cache, and carry the value in none of the scenarios' other answers", () => {
    for (const s of scenarios) {
      const reveal = s.responses.filter((r) => r.method === "POST" && r.path === REVEAL_PATH);
      expect(reveal, s.id).toHaveLength(1);
      expect(reveal[0]?.headers?.["cache-control"], s.id).toBe("no-store");
      for (const other of s.responses.filter((r) => r !== reveal[0])) expect(JSON.stringify(resolveBody(other, contextFor(other)) ?? null), `${s.id} ${other.path}`).not.toContain(REVEALED_VALUE);
    }
  });

  it("gives the limit and the unavailable answers a wait", () => {
    const wait = (id: string) => scenarios.find((s) => s.id === id)?.responses.find((r) => r.path === REVEAL_PATH)?.headers?.["retry-after"];
    expect(wait("design-masking-throttled")).toBe("30");
    expect(wait("design-masking-unavailable")).toBe("5");
  });

  it("has a multi-mask file whose masks the plan accepts: one names its setting, the other two do not", () => {
    const { content } = multiMaskFile();
    const plan = planMasks(content.text ?? "", content.masks);
    expect(plan.status).toBe("ok");
    expect(content.masks).toHaveLength(2);
    expect(content.masks?.[0]?.setting).toBe(SECRET_SETTING);
    expect(content.masks?.[1]?.setting).toBeUndefined();
    // The third marker is not in the list: it is recognised by its form.
    expect(content.text).toContain("[REDACTED: 12 chars, jwt]");
  });
});
