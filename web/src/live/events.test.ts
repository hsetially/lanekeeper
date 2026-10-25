import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { parse as parseYaml } from "yaml";
import { EventKind } from "@/api/generated/model";
import { EVENT_KINDS, parseDomainEvent } from "./events";

// What the stream carries is untrusted input (S11): every frame is parsed and checked against the contract's own field
// list before anything reads it, and a frame that does not fit is dropped, never half-used.

const spec = parseYaml(readFileSync(new URL("../../../api/openapi.yaml", import.meta.url), "utf8")) as {
  components: { schemas: Record<string, { properties?: Record<string, unknown>; required?: string[]; oneOf?: Array<{ $ref: string }> }> };
};

const SAMPLES: Record<string, Record<string, unknown>> = {
  file_observed: { type: "file_observed", swimlane: "sit1", path: "core/a.yml", hash: "a".repeat(64), source: "scan" },
  drift_changed: { type: "drift_changed", swimlane: "sit1", paths: ["core/a.yml", "core/b.yml"] },
  findings_changed: { type: "findings_changed", swimlane: "sit1" },
  pending_restart_changed: { type: "pending_restart_changed", swimlane: "sit1", services: [{ namespace: "sit1", name: "core" }] },
  proposal_changed: { type: "proposal_changed", id: 12, status: "approved" },
  agent_status_changed: { type: "agent_status_changed", swimlane: "sit1", status: "connected" },
  git_head_moved: { type: "git_head_moved", repo: "base", branch: "main", from: null, to: "b".repeat(40) },
  access_request_created: { type: "access_request_created", user: "u1" },
  doc_indexed: { type: "doc_indexed", doc: 3 },
  resync: { type: "resync" },
  attribution_updated: { type: "attribution_updated", swimlane: "sit1", path: "core/a.yml", hash: "a".repeat(64) },
  pr_state_changed: { type: "pr_state_changed", link_id: 5, state: "merged" },
  pr_job_progress: { type: "pr_job_progress", job_id: 9, state: "opening" },
  sync_window_opened: { type: "sync_window_opened", swimlane: "sit1", job: { name: "sync-1", uid: "abc-1" } },
  sync_window_closed: { type: "sync_window_closed", swimlane: "sit1", job: { name: "sync-1", uid: "abc-1" } },
  pickup_changed: { type: "pickup_changed", swimlane: "sit1", services: [] },
  swimlane_changed: { type: "swimlane_changed", swimlane: "sit1" },
};

describe("the event kinds", () => {
  it("are exactly the public EventKind values of the contract, all of them", () => {
    expect([...EVENT_KINDS].sort()).toEqual(Object.values(EventKind).sort());
    expect(Object.keys(SAMPLES).sort()).toEqual([...EVENT_KINDS].sort());
  });

  it("are the kinds of the DomainEvent schema, and every field the schema requires is one the validator checks", () => {
    const schemas = spec.components.schemas;
    const refs = schemas["DomainEvent"]?.oneOf ?? [];
    expect(refs).toHaveLength(EVENT_KINDS.length);
    for (const { $ref } of refs) {
      const schema = schemas[$ref.split("/").pop() ?? ""];
      const required = schema?.required ?? [];
      const type = (schema?.properties?.["type"] as { const?: string } | undefined)?.const ?? "";
      expect(SAMPLES[type], type).toBeDefined();
      expect([...required].sort(), type).toEqual(Object.keys(SAMPLES[type] ?? {}).sort());
    }
  });
});

describe("parseDomainEvent", () => {
  it.each(Object.entries(SAMPLES))("accepts a well-formed %s", (_kind, sample) => {
    expect(parseDomainEvent(JSON.stringify(sample))).toEqual(sample);
  });

  it.each(Object.entries(SAMPLES).filter(([, s]) => Object.keys(s).length > 1))("refuses %s without a field", (_kind, sample) => {
    for (const field of Object.keys(sample).filter((f) => f !== "type")) {
      const { [field]: _gone, ...rest } = sample;
      expect(parseDomainEvent(JSON.stringify(rest)), field).toBeNull();
    }
  });

  it("refuses a field of the wrong type", () => {
    expect(parseDomainEvent(JSON.stringify({ ...SAMPLES["file_observed"], path: 5 }))).toBeNull();
    expect(parseDomainEvent(JSON.stringify({ ...SAMPLES["proposal_changed"], id: "12" }))).toBeNull();
    expect(parseDomainEvent(JSON.stringify({ ...SAMPLES["drift_changed"], paths: "core/a.yml" }))).toBeNull();
    expect(parseDomainEvent(JSON.stringify({ ...SAMPLES["drift_changed"], paths: ["a", 1] }))).toBeNull();
    expect(parseDomainEvent(JSON.stringify({ ...SAMPLES["git_head_moved"], from: 3 }))).toBeNull();
  });

  it("refuses an unknown kind, a missing type, and anything that is not a JSON object", () => {
    for (const text of [JSON.stringify({ type: "something_new", swimlane: "x" }), "{}", "[]", "null", "5", '"x"', "not json", "", "{"]) {
      expect(parseDomainEvent(text), text).toBeNull();
    }
  });

  it("refuses a list larger than the contract allows (1,000), so a huge frame cannot become a huge loop", () => {
    expect(parseDomainEvent(JSON.stringify({ type: "drift_changed", swimlane: "sit1", paths: Array.from({ length: 1001 }, (_, i) => `p${i}`) }))).toBeNull();
    expect(parseDomainEvent(JSON.stringify({ type: "drift_changed", swimlane: "sit1", paths: Array.from({ length: 1000 }, (_, i) => `p${i}`) }))).not.toBeNull();
  });

  it("reads a key that looks like a prototype pollution attempt as plain data", () => {
    const parsed = parseDomainEvent('{"type":"findings_changed","swimlane":"sit1","__proto__":{"polluted":true}}');
    expect(parsed).not.toBeNull();
    expect(({} as Record<string, unknown>)["polluted"]).toBeUndefined();
  });
});
