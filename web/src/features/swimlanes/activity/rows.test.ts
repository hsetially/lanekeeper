import { describe, expect, it } from "vitest";
import type { AuditEvent } from "@/api/generated/model";
import { actorLabel, newestFirst, viaLabel } from "./rows";

const HASH = "a".repeat(64);
const event = (id: number, over: Partial<AuditEvent> = {}): AuditEvent => ({
  id,
  seq: id,
  at: 1_000 + id,
  action: "file.edit",
  actor: "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
  via: "ui",
  swimlane: "sitb",
  target: "tx-infinity-api/core.yml",
  request_id: null,
  attribution: null,
  diff: null,
  prev_hash: HASH,
  hash: HASH,
  ...over,
});

describe("newestFirst", () => {
  it("orders by time, newest first, and by id when the time is the same", () => {
    expect(newestFirst([event(1), event(3), event(2, { at: 5_000 }), event(4, { at: 1_003 })]).map((e) => e.id)).toEqual([2, 4, 3, 1]);
  });

  it("does not change the list it was given", () => {
    const input = [event(1), event(2)];
    newestFirst(input);
    expect(input.map((e) => e.id)).toEqual([1, 2]);
  });
});

describe("actorLabel", () => {
  it("says Lanekeeper for a change nobody made by hand", () => {
    expect(actorLabel(null)).toBe("Lanekeeper");
  });

  it("shortens a user id to the start of the person's own id, because the audit read carries no name", () => {
    expect(actorLabel("11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa")).toBe("user aaaaaaaa");
  });

  it("keeps whatever it cannot read as it is, shortened", () => {
    expect(actorLabel("x".repeat(40))).toBe(`user ${"x".repeat(8)}`);
  });
});

describe("viaLabel", () => {
  it("names the three ways in", () => {
    expect([viaLabel("ui"), viaLabel("mcp"), viaLabel("system")]).toEqual(["Web", "MCP", "System"]);
  });

  it("says Other for a way it does not know, rather than guess", () => {
    expect(viaLabel("sync" as never)).toBe("Other");
  });
});
