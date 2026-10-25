import { describe, expect, it } from "vitest";
import type { SentinelInfo } from "@/api/generated/model";
import { sentinel } from "@/mocks/design/admin-t5/fixtures";
import { backlogText, countText, disconnected, sentinelView } from "./sentinels-model";

// How a sentinel's row reads. The status is the hub's word (connected, disconnected, never seen); nothing here decides that a
// sentinel is silent from the age of its last-seen time, because no threshold is in the contract, and that time is the sentinel's
// last hello or batch of records, not a heartbeat (D123).

describe("sentinelView", () => {
  it("says a connected sentinel is connected", () => {
    expect(sentinelView("connected")).toMatchObject({ label: "Connected", tone: "ok", connected: true });
  });

  it("uses the contract's words for the other two: disconnected, and never seen", () => {
    expect(sentinelView("disconnected")).toMatchObject({ label: "Disconnected", tone: "gap", connected: false });
    expect(sentinelView("never_seen")).toMatchObject({ label: "Never seen", tone: "unknown", connected: false });
  });

  it("never says connected about a status it does not know", () => {
    const view = sentinelView("sleeping" as SentinelInfo["status"]);
    expect(view.connected).toBe(false);
    expect(view.label).not.toBe("Connected");
  });
});

describe("disconnected", () => {
  it("lists the sentinels that were connected and are not, in the order they came, and not the ones never seen", () => {
    const rows = [sentinel("a"), sentinel("b", { status: "disconnected" }), sentinel("c", { status: "never_seen", last_seen_at: null }), sentinel("d", { status: "disconnected" })];
    expect(disconnected(rows).map((r) => r.host)).toEqual(["b", "d"]);
  });
});

describe("countText", () => {
  it("counts what is registered, how many are disconnected and how many were never seen", () => {
    expect(countText([sentinel("a"), sentinel("b", { status: "disconnected" }), sentinel("c", { status: "never_seen", last_seen_at: null })])).toBe("3 registered · 1 disconnected · 1 never seen");
    expect(countText([sentinel("a")])).toBe("1 registered");
    expect(countText([sentinel("a", { status: "disconnected" }), sentinel("b", { status: "disconnected" })])).toBe("2 registered · 2 disconnected");
  });
});

describe("backlogText", () => {
  it("says nothing waiting, or how many records wait on the VM", () => {
    expect(backlogText(0)).toBe("None");
    expect(backlogText(1)).toBe("1 record");
    expect(backlogText(1200)).toBe("1,200 records");
  });
});
