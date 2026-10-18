import { describe, expect, it } from "vitest";
import type { PickupState, ProposalStatus } from "@/api/generated/model";
import { cannotApply, closedBy, pickupVariant } from "./model";

const STATUSES: ProposalStatus[] = ["draft", "pending", "approved", "rejected", "stale", "expired", "applied"];

describe("closedBy", () => {
  it("says nothing for a draft that can be applied, and something for every other status", () => {
    expect(closedBy("draft")).toBeNull();
    for (const status of STATUSES.filter((s) => s !== "draft")) {
      const closed = closedBy(status);
      expect(closed, status).not.toBeNull();
      expect(closed?.title.length).toBeGreaterThan(0);
    }
  });

  it("calls an applied draft a success and a rejected, stale or expired one a warning", () => {
    expect(closedBy("applied")?.tone).toBe("success");
    for (const status of ["rejected", "stale", "expired"] as const) expect(closedBy(status)?.tone).toBe("warning");
  });

  it("says nothing was applied for a draft that is waiting", () => {
    expect(closedBy("pending")?.text).toMatch(/Nothing has changed on NFS yet/);
  });
});

describe("cannotApply", () => {
  it("allows only a draft", () => {
    expect(cannotApply("draft")).toBeNull();
    for (const status of STATUSES.filter((s) => s !== "draft")) expect(cannotApply(status), status).not.toBeNull();
  });
});

describe("pickupVariant", () => {
  it("maps each of the four pickup states to its own badge", () => {
    const states: PickupState[] = ["live", "live_within_ttl", "needs_notify_or_restart", "needs_config_server_restart"];
    expect(states.map(pickupVariant)).toEqual(["live", "live-by", "needs-notify-or-restart", "needs-config-server-restart"]);
  });
});
