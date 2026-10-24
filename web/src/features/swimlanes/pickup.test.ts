import { describe, expect, it } from "vitest";
import type { PickupState, ServiceState } from "@/api/generated/model";
import { pendingFilesSummary, pickupSummary, pickupVariant } from "./pickup";

describe("pickupVariant (D85: four states, no bare pending-restart)", () => {
  it.each<[PickupState, string]>([
    ["live", "live"],
    ["live_within_ttl", "live-by"],
    ["needs_notify_or_restart", "needs-notify-or-restart"],
    ["needs_config_server_restart", "needs-config-server-restart"],
  ])("%s is %s", (state, variant) => {
    expect(pickupVariant(state)).toBe(variant);
  });
});

describe("pickupSummary (the Pickup column of the swimlane list)", () => {
  it("is empty when nothing waits, and the field is absent", () => {
    expect(pickupSummary(undefined)).toEqual({ actionable: 0, variant: null, withinTtl: 0, title: "Every service has picked up its config" });
    expect(pickupSummary([])).toMatchObject({ actionable: 0, variant: null });
  });

  it("counts the services that need someone to act, in the colour of the more urgent state", () => {
    const summary = pickupSummary([
      ["needs_notify_or_restart", 2],
      ["needs_config_server_restart", 1],
    ]);
    expect(summary.actionable).toBe(3);
    expect(summary.variant).toBe("needs-config-server-restart");
    expect(summary.title).toBe("1 needs config-server restart, 2 need notify or restart");
  });

  it("does not count a service that will pick it up by itself as a service that needs action", () => {
    const summary = pickupSummary([["live_within_ttl", 4]]);
    expect(summary.actionable).toBe(0);
    expect(summary.variant).toBeNull();
    expect(summary.withinTtl).toBe(4);
    expect(summary.title).toBe("4 live within their cache time");
  });

  it("adds the within-cache count to the title beside the others", () => {
    expect(pickupSummary([["needs_notify_or_restart", 1], ["live_within_ttl", 2]]).title).toBe("1 needs notify or restart, 2 live within their cache time");
  });

  it("ignores a count that is not a whole positive number", () => {
    expect(pickupSummary([["needs_notify_or_restart", -1], ["needs_config_server_restart", Number.NaN]]).actionable).toBe(0);
  });
});

const service = (over: Partial<ServiceState> = {}): ServiceState => ({
  service: { namespace: "sitb", name: "tx-infinity-core" },
  pickup: "needs_notify_or_restart",
  pending_since: 1,
  live_by: null,
  notifications_enabled: true,
  cache_ttl_secs: 300,
  pending_files: [],
  ...over,
});

describe("pendingFilesSummary", () => {
  it("is nothing for a service with no waiting file", () => {
    expect(pendingFilesSummary(service())).toEqual({ first: null, more: 0, all: [] });
  });

  it("names the first file by its file name and counts the rest", () => {
    const summary = pendingFilesSummary(service({ pending_files: ["tx-infinity-api/application.properties", "root.yml", "other/x.yml"] }));
    expect(summary.first).toBe("application.properties");
    expect(summary.more).toBe(2);
    expect(summary.all).toEqual(["tx-infinity-api/application.properties", "root.yml", "other/x.yml"]);
  });
});
