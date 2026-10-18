import { describe, expect, it } from "vitest";
import type { Attribution, AuditEvent } from "@/api/generated/model";
import { actionView, applyQuick, auditParams, chainOf, confidenceView, diffLines, filtersFromSearch, newestFirst, quickCounts, searchOf, sourceLabel, viaView, whoOf, KNOWN_ACTIONS, PAGE_SIZE } from "./model";

const TENANT = "11111111-1111-4111-8111-111111111111";
const ME = `${TENANT}:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa`;
const OTHER = `${TENANT}:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb`;
const h = (n: number): string => n.toString(16).padStart(64, "0");

const event = (seq: number, over: Partial<AuditEvent> = {}): AuditEvent => ({
  id: seq,
  seq,
  at: 1_760_000_000_000 + seq,
  action: "file_edited",
  actor: ME,
  via: "ui",
  swimlane: "sitb",
  target: "tx-infinity-api/core.yml",
  request_id: "req-1",
  attribution: null,
  diff: null,
  prev_hash: h(seq - 1),
  hash: h(seq),
  ...over,
});

const attribution = (over: Partial<Attribution> = {}): Attribution => ({ source: "tool_write", confidence: "certain", actor: { kind: "user", user: ME }, evidence: {}, ...over });
const name = (id: string): string => (id === ME ? "Priya Shah" : id === OTHER ? "Amir Haddad" : "someone");

describe("what an action is called", () => {
  it("names the actions the hub writes", () => {
    expect(actionView("file_edited").label).toBe("Edit");
    expect(actionView("file_uploaded").label).toBe("Upload");
    expect(actionView("file_deleted").label).toBe("Delete");
    expect(actionView("service_restarted").label).toBe("Restart");
    expect(actionView("proposal_approved").label).toBe("Approve");
    expect(actionView("proposal_rejected").label).toBe("Reject");
    expect(actionView("flagged_value_revealed").label).toBe("Reveal");
    expect(actionView("attribution_upgraded").label).toBe("Attribution upgraded");
    expect(actionView("pr_raised").label).toBe("Pull request");
  });

  it("makes a readable name of one it does not know, and never loses it", () => {
    expect(actionView("some_new_action").label).toBe("Some new action");
    expect(actionView("x".repeat(300)).label.length).toBeGreaterThan(0);
  });

  it("lists the known actions for a filter, once each, in alphabetical order of their names", () => {
    expect(KNOWN_ACTIONS).toContain("file_edited");
    expect(new Set(KNOWN_ACTIONS).size).toBe(KNOWN_ACTIONS.length);
    expect([...KNOWN_ACTIONS]).toEqual([...KNOWN_ACTIONS].sort());
  });
});

describe("how the event arrived", () => {
  it("is UI, Cursor (the MCP server), or something the system did", () => {
    expect(viaView(event(1)).label).toBe("UI");
    expect(viaView(event(1, { via: "mcp" })).label).toBe("Cursor");
    expect(viaView(event(1, { via: "system" })).label).toBe("System");
  });

  it("tells a sync job and a change found on the NFS volume from the rest of the system", () => {
    expect(viaView(event(1, { via: "system", attribution: attribution({ source: "sync_job" }) })).label).toBe("Sync");
    for (const source of ["sentinel_login", "nfs_client", "fs_owner_hint", "unknown"] as const) {
      expect(viaView(event(1, { via: "system", attribution: attribution({ source }) })).label, source).toBe("Detected");
    }
  });
});

describe("who made the change", () => {
  it("is the actor of the event when nothing else is known", () => {
    expect(whoOf(event(1), name)).toEqual({ kind: "named", name: "Priya Shah", probable: false });
    expect(whoOf(event(1, { actor: null }), name)).toEqual({ kind: "system" });
  });

  it("is the person the attribution names, and only probably so when it is not sure", () => {
    const sure = event(1, { via: "system", actor: null, attribution: attribution({ source: "sentinel_login", confidence: "high", actor: { kind: "user", user: OTHER } }) });
    expect(whoOf(sure, name)).toEqual({ kind: "named", name: "Amir Haddad", probable: false });
    const guess = event(1, { via: "system", actor: null, attribution: attribution({ source: "nfs_client", confidence: "medium", actor: { kind: "user", user: OTHER } }) });
    expect(whoOf(guess, name)).toEqual({ kind: "named", name: "Amir Haddad", probable: true });
    const login = event(1, { actor: null, attribution: attribution({ source: "sentinel_login", confidence: "low", actor: { kind: "os_login", login: "dana_okoye_corp_example" } }) });
    expect(whoOf(login, name)).toEqual({ kind: "named", name: "dana_okoye_corp_example", probable: true });
    const job = event(1, { actor: null, attribution: attribution({ source: "sync_job", actor: { kind: "job", job: { name: "sit7-sync", uid: "7731" } } }) });
    expect(whoOf(job, name)).toEqual({ kind: "named", name: "sit7-sync", probable: false });
  });

  it("is unknown when the attribution says so or names no one", () => {
    expect(whoOf(event(1, { actor: null, attribution: attribution({ source: "unknown", confidence: "low", actor: null }) }), name)).toEqual({ kind: "unknown" });
    expect(whoOf(event(1, { actor: null, attribution: attribution({ source: "nfs_client", confidence: "low", actor: null }) }), name)).toEqual({ kind: "unknown" });
  });

  it("falls back to the actor of the event for a change the tool itself wrote and names no one", () => {
    expect(whoOf(event(1, { attribution: attribution({ source: "tool_write", actor: null }) }), name)).toEqual({ kind: "named", name: "Priya Shah", probable: false });
  });

  it("names the author of a change the tool wrote, not the person who recorded it (the approver is the actor of the event)", () => {
    const approved = event(1, { actor: ME, attribution: attribution({ source: "tool_write", actor: { kind: "user", user: OTHER } }) });
    expect(whoOf(approved, name)).toEqual({ kind: "named", name: "Amir Haddad", probable: false });
  });
});

describe("how sure the attribution is", () => {
  it("has four levels, a word and a bar for each, and calls the two lower ones probable", () => {
    expect(confidenceView("certain")).toEqual({ label: "Certain", bar: 100, low: false });
    expect(confidenceView("high")).toEqual({ label: "High", bar: 75, low: false });
    expect(confidenceView("medium")).toEqual({ label: "Medium", bar: 50, low: true });
    expect(confidenceView("low")).toEqual({ label: "Low", bar: 25, low: true });
  });

  it("names every source", () => {
    expect(sourceLabel("tool_write")).toBe("Via Lanekeeper");
    expect(sourceLabel("sentinel_login")).toBe("Edited on the NFS VM");
    expect(sourceLabel("sync_job")).toBe("Sync job");
    expect(sourceLabel("nfs_client")).toBe("NFS client");
    expect(sourceLabel("fs_owner_hint")).toBe("File owner hint");
    expect(sourceLabel("unknown")).toBe("Unknown");
  });
});

describe("the filters", () => {
  it("are read from the address, and anything that is not what the hub takes is dropped", () => {
    expect(filtersFromSearch({ actor: ME, swimlane: "sitb", action: "file_edited", from: "2025-10-08", to: "2025-10-09" })).toEqual({ actor: ME, swimlane: "sitb", action: "file_edited", from: "2025-10-08", to: "2025-10-09" });
    expect(filtersFromSearch({ actor: "not-a-user", swimlane: "Sit B!", action: "x".repeat(257), from: "yesterday", to: "2025-13-45" })).toEqual({});
    expect(filtersFromSearch({})).toEqual({});
  });

  it("go to the hub as a day's first and last millisecond in UTC", () => {
    expect(auditParams({ swimlane: "sitb", from: "2025-10-08", to: "2025-10-09" })).toEqual({ swimlane: "sitb", from: Date.UTC(2025, 9, 8), to: Date.UTC(2025, 9, 9) + 86_400_000 - 1, limit: PAGE_SIZE });
    expect(auditParams({})).toEqual({ limit: PAGE_SIZE });
  });

  it("go back to the address in the order the route names them, with nothing that is empty", () => {
    expect(searchOf({ swimlane: "sitb", actor: ME })).toEqual({ actor: ME, swimlane: "sitb" });
    expect(searchOf({})).toEqual({});
  });
});

describe("the quick filters (on the events that are loaded)", () => {
  const rows = [
    event(5, { via: "mcp" }),
    event(4, { via: "system", actor: null, attribution: attribution({ source: "unknown", confidence: "low", actor: null }) }),
    event(3),
  ];

  it("count and keep the events made through Cursor and those with an unknown author", () => {
    expect(quickCounts(rows)).toEqual({ all: 3, cursor: 1, unknown: 1 });
    expect(applyQuick(rows, "all")).toHaveLength(3);
    expect(applyQuick(rows, "cursor").map((e) => e.seq)).toEqual([5]);
    expect(applyQuick(rows, "unknown").map((e) => e.seq)).toEqual([4]);
  });
});

describe("the link between events", () => {
  it("is whole when each event stores the hash of the one before it", () => {
    const rows = newestFirst([event(3), event(2), event(1)]);
    expect(chainOf(rows)).toEqual({ checked: 2, loaded: 3, breaks: new Map() });
  });

  it("is broken where an event stores another hash than the one before it has, and says both", () => {
    const rows = newestFirst([event(3), event(2, { prev_hash: h(999) }), event(1)]);
    const chain = chainOf(rows);
    expect(chain.checked).toBe(2);
    expect([...chain.breaks.keys()]).toEqual([2]);
    expect(chain.breaks.get(2)).toEqual({ stored: h(999), actual: h(1), previous: 1 });
  });

  it("judges only events that follow each other: a gap in what is loaded is not a break", () => {
    const rows = newestFirst([event(9), event(8), event(3), event(2)]);
    expect(chainOf(rows)).toEqual({ checked: 2, loaded: 4, breaks: new Map() });
  });
});

describe("the order", () => {
  it("is newest first by sequence number, whatever order the pages came in", () => {
    expect(newestFirst([event(1), event(3), event(2)]).map((e) => e.seq)).toEqual([3, 2, 1]);
  });
});

describe("the diff text of an event", () => {
  it("is shown line by line, with the hunk headers, the added and the removed lines marked", () => {
    const text = "--- a/x.yml\n+++ b/x.yml\n@@ -1,2 +1,2 @@\n a: 1\n-b: 2\n+b: 3\n";
    expect(diffLines(text, 100)).toEqual({
      lines: [
        { kind: "meta", text: "--- a/x.yml" },
        { kind: "meta", text: "+++ b/x.yml" },
        { kind: "hunk", text: "@@ -1,2 +1,2 @@" },
        { kind: "context", text: " a: 1" },
        { kind: "removed", text: "-b: 2" },
        { kind: "added", text: "+b: 3" },
      ],
      more: 0,
    });
  });

  it("is cut at the number of lines asked for and says how many were left out", () => {
    const text = Array.from({ length: 12 }, (_, i) => `+line ${i}`).join("\n");
    const cut = diffLines(text, 5);
    expect(cut.lines).toHaveLength(5);
    expect(cut.more).toBe(7);
  });

  it("keeps a line of any length as text, never as markup", () => {
    const cut = diffLines("+<img src=x onerror=alert(1)>", 5);
    expect(cut.lines).toEqual([{ kind: "added", text: "+<img src=x onerror=alert(1)>" }]);
  });
});
