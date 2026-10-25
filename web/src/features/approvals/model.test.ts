import { describe, expect, it } from "vitest";
import type { Proposal } from "@/api/generated/model";
import { actionView, canDecide, consequence, decisionLine, expiresIn, fileParts, lifetimeHours, QUEUE_LIMIT, queueParams, statusView, tabOfSearch, tabOfStatus, TABS, titleOf } from "./model";

const ME = "11111111-1111-4111-8111-111111111111:aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
const OTHER = "11111111-1111-4111-8111-111111111111:bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
const HASH = "a".repeat(64);
const NOW = 1_760_000_000_000;
const H = 3_600_000;

const proposal = (over: Partial<Proposal> = {}): Proposal => ({
  id: 2046,
  author: OTHER,
  swimlane: "sitb",
  path: "tx-infinity-api/tx-infinity-core-sit7.yml",
  expected: { state: "hash", hash: HASH },
  new_hash: "b".repeat(64),
  status: "pending",
  hunks: [],
  setting_changes: [],
  severity: null,
  created_at: NOW - 12 * 60_000,
  expires_at: NOW + 71 * H,
  decided_by: null,
  decision_comment: null,
  ...over,
});

describe("the tabs", () => {
  it("are the page's four, in the page's order", () => {
    expect(TABS).toEqual(["pending", "stale", "expired", "decided"]);
  });

  it("read the address: the page's own four, and the handoff route's approved and rejected as Decided", () => {
    expect(tabOfSearch("stale")).toBe("stale");
    expect(tabOfSearch("expired")).toBe("expired");
    expect(tabOfSearch("decided")).toBe("decided");
    expect(tabOfSearch("approved")).toBe("decided");
    expect(tabOfSearch("rejected")).toBe("decided");
    expect(tabOfSearch("applied")).toBe("decided");
  });

  it("fall back to Pending for anything else, including the handoff's withdrawn (no such status)", () => {
    expect(tabOfSearch(undefined)).toBe("pending");
    expect(tabOfSearch("withdrawn")).toBe("pending");
    expect(tabOfSearch("<script>")).toBe("pending");
  });

  it("file each proposal status under one tab", () => {
    expect(tabOfStatus("pending")).toBe("pending");
    expect(tabOfStatus("draft")).toBe("pending");
    expect(tabOfStatus("stale")).toBe("stale");
    expect(tabOfStatus("expired")).toBe("expired");
    expect(tabOfStatus("approved")).toBe("decided");
    expect(tabOfStatus("applied")).toBe("decided");
    expect(tabOfStatus("rejected")).toBe("decided");
  });
});

describe("the question each tab asks the server", () => {
  it("is the one the sidebar badge asks for Pending, so both share one cache entry", () => {
    expect(queueParams("pending")).toEqual({ status: "pending", limit: QUEUE_LIMIT });
    expect(QUEUE_LIMIT).toBe(100);
  });

  it("is a status for Stale and Expired, and the contract's `decided=true` for Decided (never a `decided` status)", () => {
    expect(queueParams("stale")).toEqual({ status: "stale", limit: 100 });
    expect(queueParams("expired")).toEqual({ status: "expired", limit: 100 });
    expect(queueParams("decided")).toEqual({ decided: true, limit: 100 });
    expect(queueParams("decided")).not.toHaveProperty("status");
  });
});

describe("what a proposal is", () => {
  it("names its action, and calls an edit made through MCP an AI-made edit", () => {
    expect(actionView(proposal({ action: "edit", via: "ui" })).label).toBe("Edit");
    expect(actionView(proposal({ action: "edit", via: "mcp" })).label).toBe("AI-made edit");
    expect(actionView(proposal({ action: "upload" })).label).toBe("Upload");
    expect(actionView(proposal({ action: "delete" })).label).toBe("Delete");
    expect(actionView(proposal({ action: "revert" })).label).toBe("Revert");
    expect(actionView(proposal({ action: "restart" })).label).toBe("Restart");
    expect(actionView(proposal({ action: "pr" })).label).toBe("Pull request");
    expect(actionView(proposal({ action: "notify" })).label).toBe("Notify");
  });

  it("says Change, and nothing more, when the server did not say what the proposal is for", () => {
    expect(actionView(proposal()).label).toBe("Change");
  });

  it("gives every status a label of its own (colour is never the only sign)", () => {
    const labels = (["draft", "pending", "approved", "rejected", "stale", "expired", "applied"] as const).map((status) => statusView(status).label);
    expect(labels).toEqual(["Draft", "Pending approval", "Approved", "Rejected", "Stale", "Expired", "Applied"]);
    expect(new Set(labels).size).toBe(labels.length);
  });
});

describe("what approving does", () => {
  it("says what the proposal will do from what the proposal says, and claims nothing it does not know", () => {
    expect(consequence(proposal({ action: "edit" }), "sitb")).toBe("Writes the proposed content to tx-infinity-api/tx-infinity-core-sit7.yml on sitb.");
    expect(consequence(proposal({ action: "upload" }), "sitb")).toBe("Replaces tx-infinity-api/tx-infinity-core-sit7.yml on sitb with the uploaded file.");
    expect(consequence(proposal({ action: "delete" }), "sitb")).toBe("Deletes tx-infinity-api/tx-infinity-core-sit7.yml from sitb.");
    expect(consequence(proposal({ action: "revert" }), "sitb")).toBe("Writes an earlier version back to tx-infinity-api/tx-infinity-core-sit7.yml on sitb.");
    expect(consequence(proposal({ action: "restart", path: "sitb/gateway", paths: ["sitb/gateway"] }), "sitb")).toBe("Restarts sitb/gateway on sitb.");
    expect(consequence(proposal({ action: "restart", path: "sitb/gateway", paths: ["sitb/gateway", "sitb/core"] }), "sitb")).toBe("Restarts 2 services on sitb.");
    expect(consequence(proposal({ action: "pr" }), "sitb")).toBe("Opens a pull request. Nothing changes on sitb until it merges.");
    expect(consequence(proposal({ action: "notify" }), "sitb")).toBe("Asks the config server to pick up the change on sitb.");
    expect(consequence(proposal(), "sitb")).toBe("Applies the proposed change to sitb.");
  });

  it("says that a stale proposal can not be approved, and an expired one is read only", () => {
    expect(consequence(proposal({ status: "stale" }), "sitb")).toBe("Not approvable: the file changed since this was proposed.");
    expect(consequence(proposal({ status: "expired" }), "sitb")).toBe("Would have changed sitb. It expired with no decision, so nothing was written.");
  });

  it("says what a decision did", () => {
    expect(consequence(proposal({ status: "rejected" }), "sitb")).toBe("Nothing was written to sitb.");
    expect(consequence(proposal({ status: "applied" }), "sitb")).toBe("Written to sitb on approval.");
    expect(consequence(proposal({ status: "approved" }), "sitb")).toBe("Approved. The write to sitb follows the approval.");
  });
});

describe("when a proposal expires", () => {
  it("says how long is left, and marks the last six hours as urgent", () => {
    expect(expiresIn(proposal({ expires_at: NOW + 23 * H }), NOW)).toEqual({ text: "23 h", urgent: false, over: false });
    expect(expiresIn(proposal({ expires_at: NOW + 5 * H }), NOW)).toEqual({ text: "5 h", urgent: true, over: false });
    expect(expiresIn(proposal({ expires_at: NOW + 40 * 60_000 }), NOW)).toEqual({ text: "40 min", urgent: true, over: false });
  });

  it("says how long ago it ran out", () => {
    expect(expiresIn(proposal({ expires_at: NOW - 6 * H }), NOW)).toEqual({ text: "6 h ago", urgent: false, over: true });
  });

  it("reads the lifetime from the proposal itself (72 hours, not the design's 24)", () => {
    expect(lifetimeHours(proposal({ created_at: NOW, expires_at: NOW + 72 * H }))).toBe(72);
    expect(lifetimeHours(proposal({ created_at: NOW, expires_at: NOW + 24 * H }))).toBe(24);
    expect(lifetimeHours(proposal({ created_at: NOW, expires_at: NOW }))).toBeNull();
  });
});

describe("who decided and how", () => {
  it("reads decided_by, the comment and the time", () => {
    const rejected = proposal({ status: "rejected", decided_by: OTHER, decision_comment: "Wrong swimlane.", decided_at: NOW - H });
    expect(decisionLine(rejected)).toEqual({ by: OTHER, verb: "Rejected", reason: "Wrong swimlane.", at: NOW - H });
    expect(decisionLine(proposal({ status: "applied", decided_by: OTHER }))).toEqual({ by: OTHER, verb: "Approved", reason: null, at: null });
  });

  it("has no decision for a proposal that is not decided", () => {
    expect(decisionLine(proposal())).toBeNull();
    expect(decisionLine(proposal({ status: "expired" }))).toBeNull();
  });
});

describe("who may decide a proposal (the server decides; this explains)", () => {
  it("lets an Admin approve and reject another person's pending proposal", () => {
    expect(canDecide(proposal(), ME)).toEqual({ approve: { allowed: true, reason: null }, reject: { allowed: true, reason: null }, note: null });
  });

  it("refuses a person their own proposal, both ways, and says who else can", () => {
    const own = canDecide(proposal({ author: ME }), ME);
    expect(own.approve).toEqual({ allowed: false, reason: "You can't approve your own proposal" });
    expect(own.reject).toEqual({ allowed: false, reason: "You can't reject your own proposal" });
    expect(own.note).toBe("own");
  });

  it("allows only Reject on a stale proposal", () => {
    const stale = canDecide(proposal({ status: "stale" }), ME);
    expect(stale.approve).toEqual({ allowed: false, reason: "The file changed since this was proposed" });
    expect(stale.reject.allowed).toBe(true);
    expect(stale.note).toBe("stale");
  });

  it("allows nothing on an expired or decided proposal", () => {
    for (const status of ["expired", "approved", "applied", "rejected"] as const) {
      const none = canDecide(proposal({ status }), ME);
      expect(none.approve.allowed, status).toBe(false);
      expect(none.reject.allowed, status).toBe(false);
    }
    expect(canDecide(proposal({ status: "expired" }), ME).note).toBe("expired");
    expect(canDecide(proposal({ status: "applied" }), ME).note).toBe("decided");
  });

  it("does not let a stale proposal of your own be rejected either", () => {
    expect(canDecide(proposal({ status: "stale", author: ME }), ME).reject.allowed).toBe(false);
  });
});

describe("what a proposal is called", () => {
  it("names the file and its folder, or the services of a restart", () => {
    expect(fileParts(proposal())).toEqual({ name: "tx-infinity-core-sit7.yml", where: "tx-infinity-api/" });
    expect(fileParts(proposal({ path: "a.yml" }))).toEqual({ name: "a.yml", where: "" });
    expect(fileParts(proposal({ action: "restart", path: "sitb/gateway", paths: ["sitb/gateway", "sitb/core"] }))).toEqual({ name: "gateway, core", where: "services/" });
  });

  it("titles the drawer by what the proposal does", () => {
    expect(titleOf(proposal({ action: "edit" }))).toBe("Edit tx-infinity-core-sit7.yml");
    expect(titleOf(proposal({ action: "edit", via: "mcp" }))).toBe("AI-made edit to tx-infinity-core-sit7.yml");
    expect(titleOf(proposal({ action: "pr" }))).toBe("Pull request for tx-infinity-core-sit7.yml");
    expect(titleOf(proposal({ action: "restart", path: "sitb/gateway", paths: ["sitb/gateway"] }))).toBe("Restart gateway");
    expect(titleOf(proposal())).toBe("Change to tx-infinity-core-sit7.yml");
  });
});
