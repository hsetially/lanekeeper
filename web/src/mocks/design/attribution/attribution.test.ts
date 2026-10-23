import { readFileSync } from "node:fs";
import Ajv2020 from "ajv/dist/2020.js";
import { describe, expect, it } from "vitest";
import { parse } from "yaml";
import { resolveBody, contextFor, type MockContext, type MockResponse } from "../../scenario.ts";
import { collectDesignScenarios } from "../index.ts";
import { auditEvents, BY_SOURCE, historyVersions, hostileVersions } from "./data.ts";
import { scenarios } from "./index.ts";

// The scenarios of the attribution slice. The loader (../index.ts) is shared and lists the slice folders; until it lists this one,
// src/mocks/mocks.test.ts does not see these scenarios, so this file checks them against the OpenAPI schemas itself, the same
// way: every response must be one the server could give.

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
  const key = operation.responses[String(status)] ? String(status) : "default";
  const entry = operation.responses[key];
  if (!entry) throw new Error(`${operation.operationId} declares no ${status} response`);
  const esc = (s: string) => s.replaceAll("~", "~0").replaceAll("/", "~1");
  let base = `/paths/${esc(path)}/${method.toLowerCase()}/responses/${esc(key)}`;
  let node: { content?: Record<string, { schema?: unknown }> } = entry;
  if (entry.$ref) {
    const name = entry.$ref.split("/").pop() ?? "";
    node = doc.components.responses[name] ?? {};
    base = `/components/responses/${esc(name)}`;
  }
  const media = node.content ? Object.keys(node.content)[0] : undefined;
  return media && node.content?.[media]?.schema ? `${base}/content/${esc(media)}/schema` : undefined;
}

const all: Array<{ scenario: string; response: MockResponse; sample?: Partial<MockContext>; label: string }> = scenarios.flatMap((s) =>
  s.responses.flatMap((response) => [{ scenario: s.id, response, label: "" }, ...(response.samples ?? []).map((sample, i) => ({ scenario: s.id, response, sample, label: ` (sample ${i + 1})` }))]),
);

describe("the attribution scenarios", () => {
  it("are named for the slice, and none is used twice (the loader's own rule)", () => {
    expect(scenarios.map((s) => s.id)).toEqual(["design-attribution-history", "design-attribution-hostile", "design-attribution-audit"]);
    expect(() => collectDesignScenarios({ attribution: scenarios })).not.toThrow();
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
});

describe("the audit scenario's claim to serve a swimlane's Activity tab", () => {
  // The scenario says it is "the Audit log (and a swimlane's Activity tab)". The Activity tab reads the swimlane itself and its tree
  // before it reads the audit log, so a scenario that answers only the audit log leaves the page at "Couldn't load activity".
  it("answers the reads that /swimlanes/{id}/activity makes", () => {
    const audit = scenarios.find((s) => s.id === "design-attribution-audit");
    const answered = new Set((audit?.responses ?? []).map((r) => `${r.method} ${r.path}`));
    for (const read of ["GET /api/v1/me", "GET /api/v1/swimlanes", "GET /api/v1/swimlanes/{id}", "GET /api/v1/swimlanes/{id}/tree", "GET /api/v1/admin/users", "GET /api/v1/admin/audit"]) {
      expect(answered, read).toContain(read);
    }
  });
});

describe("what the data covers", () => {
  it("has every source of D73 and every level of confidence of the contract, in the history", () => {
    const seen = historyVersions().map((v) => v.attribution);
    expect(new Set(seen.map((a) => a.source))).toEqual(new Set(["tool_write", "sentinel_login", "sync_job", "nfs_client", "fs_owner_hint", "unknown"]));
    expect(new Set(seen.map((a) => a.confidence))).toEqual(new Set(["low", "medium", "high", "certain"]));
  });

  it("has every severity, and a version nobody rated", () => {
    const severities = historyVersions().map((v) => v.severity);
    expect(new Set(severities)).toEqual(new Set(["low", "medium", "high", "critical", null]));
  });

  it("carries the contract's low for the unknown source, which the app must not show", () => {
    expect(BY_SOURCE.unknown).toMatchObject({ source: "unknown", confidence: "low", actor: null });
  });

  it("has an NFS client that nobody is named for, and a sync job that is named", () => {
    expect(BY_SOURCE.client.actor).toBeNull();
    expect(BY_SOURCE.sync.actor).toMatchObject({ kind: "job" });
  });

  it("covers the same ground in the audit log, with a chain that is whole", () => {
    const events = auditEvents();
    const sources = new Set(events.flatMap((e) => (e.attribution === null ? [] : [e.attribution.source])));
    expect(sources).toEqual(new Set(["tool_write", "sentinel_login", "sync_job", "nfs_client", "fs_owner_hint", "unknown"]));
    for (let i = 0; i + 1 < events.length; i++) expect(events[i]?.prev_hash).toBe(events[i + 1]?.hash);
  });

  it("has hostile text that is really hostile: a bidi override, a zero-width character, markup and a 256-character login", () => {
    const text = JSON.stringify(hostileVersions());
    expect(text).toContain("\u202E");
    expect(text).toContain("\u200B");
    expect(text).toContain("<img src=x");
    expect(hostileVersions().some((v) => v.attribution.actor?.kind === "os_login" && v.attribution.actor.login.length === 256)).toBe(true);
  });
});
