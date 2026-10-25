import { describe, expect, it } from "vitest";
import type { PickupState } from "@/api/generated/model";
import {
  PICKUP_ORDER,
  countsToShow,
  formatTtl,
  isPickupState,
  isShownTime,
  liveByView,
  needsAction,
  notificationsText,
  pickupRank,
  pickupView,
  worstPickup,
} from "./model";

// The words and the order of the four pickup states (D85), with no markup in them: what a chip says, when it takes effect, how a
// time is read, and what is said when the hub gives something that cannot be read. Every expectation here is a sentence a person
// sees, so the tests read like the sentences.

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
// 2026-10-17 14:00:00 UTC.
const NOON = Date.UTC(2026, 9, 17, 14, 0, 0);

describe("the four states, most urgent first", () => {
  it("lists exactly the contract's four states, config-server restart first and live last", () => {
    expect(PICKUP_ORDER).toEqual(["needs_config_server_restart", "needs_notify_or_restart", "live_within_ttl", "live"]);
    expect(PICKUP_ORDER.map(pickupRank)).toEqual([0, 1, 2, 3]);
  });

  it.each(["live", "live_within_ttl", "needs_notify_or_restart", "needs_config_server_restart"])("knows %s", (state) => {
    expect(isPickupState(state)).toBe(true);
  });

  it.each([null, undefined, "", "pending_restart", "LIVE", 3, {}, ["live"], "<img src=x onerror=alert(1)>"])("does not know %j", (value) => {
    expect(isPickupState(value)).toBe(false);
  });

  it("says which two need a person to act", () => {
    expect(needsAction("needs_notify_or_restart")).toBe(true);
    expect(needsAction("needs_config_server_restart")).toBe(true);
    expect(needsAction("live_within_ttl")).toBe(false);
    expect(needsAction("live")).toBe(false);
    expect(needsAction(null)).toBe(false);
    expect(needsAction("pending_restart")).toBe(false);
  });

  it("picks the most urgent state that was stated, and nothing when nothing was", () => {
    expect(worstPickup(["live", "needs_notify_or_restart", "live_within_ttl"])).toBe("needs_notify_or_restart");
    expect(worstPickup(["live", "needs_config_server_restart", "needs_notify_or_restart"])).toBe("needs_config_server_restart");
    expect(worstPickup(["live", "live"])).toBe("live");
    expect(worstPickup([])).toBeNull();
    expect(worstPickup([null, undefined])).toBeNull();
  });

  it("ignores a state it does not know instead of calling it urgent or live", () => {
    expect(worstPickup(["bogus", "live_within_ttl"])).toBe("live_within_ttl");
    expect(worstPickup(["bogus"])).toBeNull();
  });
});

describe("a live-by time (the cache of the client)", () => {
  it("is the time of day in UTC, with the zone spelled out", () => {
    const view = liveByView(Date.UTC(2026, 9, 17, 14, 32), NOON);
    expect(view.kind).toBe("future");
    expect(view.text).toBe("14:32 UTC");
  });

  it("is rounded UP to the minute, so that 'by 14:33' is never earlier than the time the hub gave", () => {
    expect(liveByView(Date.UTC(2026, 9, 17, 14, 32, 40), NOON).text).toBe("14:33 UTC");
    expect(liveByView(Date.UTC(2026, 9, 17, 14, 32, 0, 1), NOON).text).toBe("14:33 UTC");
    expect(liveByView(Date.UTC(2026, 9, 17, 14, 59, 30), NOON).text).toBe("15:00 UTC");
  });

  it("keeps the seconds for the tooltip", () => {
    expect(liveByView(Date.UTC(2026, 9, 17, 14, 32, 40), NOON).exact).toBe("2026-10-17 14:32:40 UTC");
  });

  it("names the day when it is not today, because 00:05 alone could be either", () => {
    expect(liveByView(Date.UTC(2026, 9, 18, 0, 5), NOON).text).toBe("2026-10-18 00:05 UTC");
    expect(liveByView(Date.UTC(2026, 9, 17, 23, 59, 30), NOON).text).toBe("2026-10-18 00:00 UTC");
  });

  it("is past when the time has gone, and says so instead of presenting it as still to come", () => {
    expect(liveByView(NOON - MIN, NOON).kind).toBe("past");
    expect(liveByView(NOON, NOON).kind).toBe("past");
    expect(liveByView(NOON + 1, NOON).kind).toBe("future");
  });

  it("does not judge past or future without a clock, and still shows the time the hub gave", () => {
    const view = liveByView(Date.UTC(2026, 9, 17, 14, 32), undefined);
    expect(view.kind).toBe("future");
    expect(view.text).toBe("14:32 UTC");
  });

  it.each([null, undefined, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY, -1, 253_402_300_800_000, "1760000000000", {}, true])("has no time for %j and invents none", (value) => {
    const view = liveByView(value, NOON);
    expect(view).toEqual({ kind: "none", text: null, exact: null });
  });
});

describe("the chip of one state", () => {
  it("draws Live", () => {
    const view = pickupView("live", { subject: "service" });
    expect(view).toMatchObject({ state: "live", variant: "live", text: "Live", qualifier: null, name: "Live", action: false });
    expect(view.words).toBe("This service already has the current config.");
  });

  it("draws 'Live by 14:32 UTC (client cache)' for a resource file that is waiting on the cache", () => {
    const view = pickupView("live_within_ttl", { subject: "service", liveBy: Date.UTC(2026, 9, 17, 14, 32), now: NOON });
    expect(view.variant).toBe("live-by");
    expect(view.text).toBe("Live by 14:32 UTC");
    expect(view.qualifier).toBe("(client cache)");
    expect(view.name).toBe("Live by 14:32 UTC (client cache)");
    expect(view.words).toBe("A resource file changed. This service reads it again when its client cache expires, by 2026-10-17 14:32:00 UTC.");
    expect(view.action).toBe(false);
  });

  it("does not make up a time when the hub gave none, and says that it did not", () => {
    const view = pickupView("live_within_ttl", { subject: "service", liveBy: null, now: NOON });
    expect(view.text).toBe("Live within client cache");
    expect(view.qualifier).toBeNull();
    expect(view.name).toBe("Live within client cache");
    expect(view.words).toBe("A resource file changed. This service reads it again when its client cache expires. The hub did not say when.");
  });

  it("does not say that a time that has gone is still to come", () => {
    const view = pickupView("live_within_ttl", { subject: "service", liveBy: NOON - 5 * MIN, now: NOON });
    expect(view.text).toBe("Live by 13:55 UTC");
    expect(view.qualifier).toBe("(time passed)");
    expect(view.name).toBe("Live by 13:55 UTC (time passed)");
    expect(view.words).toBe("A resource file changed. Its client cache should have expired by 2026-10-17 13:55:00 UTC, and the hub has not reported this service as live yet.");
  });

  it("does not make up a time for a live-by that cannot be read", () => {
    for (const bad of [Number.NaN, -5, "soon"]) {
      const view = pickupView("live_within_ttl", { subject: "service", liveBy: bad, now: NOON });
      expect(view.text, String(bad)).toBe("Live within client cache");
      expect(view.words, String(bad)).toContain("The hub did not say when.");
    }
  });

  it("says nothing about 'the hub did not say when' where the time is not part of the data (a draft's list of services)", () => {
    const view = pickupView("live_within_ttl", { subject: "service" });
    expect(view.text).toBe("Live within client cache");
    expect(view.words).toBe("A resource file changed. This service reads it again when its client cache expires.");
  });

  it("draws Needs notify or restart for a property file", () => {
    const view = pickupView("needs_notify_or_restart", { subject: "service" });
    expect(view).toMatchObject({ variant: "needs-notify-or-restart", text: "Needs notify or restart", action: true });
    expect(view.words).toBe("A property file changed. This service takes it after it is notified or restarted.");
  });

  it("draws Needs config-server restart for a new folder or channels.yml", () => {
    const view = pickupView("needs_config_server_restart", { subject: "service" });
    expect(view).toMatchObject({ variant: "needs-config-server-restart", text: "Needs config-server restart", action: true });
    expect(view.words).toBe("A new folder or a change to channels.yml. The config-server does not serve it until the config-server restarts.");
  });

  it("says it of a file in the file's own words", () => {
    expect(pickupView("needs_notify_or_restart", { subject: "file" }).words).toBe("A property file: services take the new version after they are notified or restarted.");
    expect(pickupView("needs_config_server_restart", { subject: "file" }).words).toBe("In a new folder or channels.yml: the config-server does not serve it until it restarts.");
    expect(pickupView("live_within_ttl", { subject: "file" }).words).toBe("A resource file: services read the new version when their client cache expires.");
    expect(pickupView("live", { subject: "file" }).words).toBe("Nothing is waiting on this file.");
  });

  it("counts services in the words of a summary, in the singular and the plural", () => {
    expect(pickupView("needs_notify_or_restart", { subject: "count", count: 1 }).words).toBe("1 service needs a notify or a restart to take a changed property file.");
    expect(pickupView("needs_notify_or_restart", { subject: "count", count: 3 }).words).toBe("3 services need a notify or a restart to take a changed property file.");
    expect(pickupView("needs_config_server_restart", { subject: "count", count: 1 }).words).toBe("1 service waits for the config-server to restart (a new folder or a change to channels.yml).");
    expect(pickupView("needs_config_server_restart", { subject: "count", count: 2 }).words).toBe("2 services wait for the config-server to restart (a new folder or a change to channels.yml).");
    expect(pickupView("live_within_ttl", { subject: "count", count: 4 }).words).toBe("4 services read a changed resource file again when their client cache expires.");
  });

  it("never says 'pending restart' (D85: there is no bare pending restart), in any state or any place", () => {
    for (const state of PICKUP_ORDER) {
      for (const subject of ["service", "file", "count"] as const) {
        const view = pickupView(state, { subject, count: 2, liveBy: NOON + MIN, now: NOON });
        for (const text of [view.text, view.name, view.words]) expect(text, `${state} ${subject}`).not.toMatch(/pending restart/i);
      }
    }
  });

  it("says that it does not know a state it does not know, and never repeats what the hub sent", () => {
    for (const bad of ["pending_restart", "<img src=x onerror=alert(1)>", "live‮", "\u0000"]) {
      const view = pickupView(bad, { subject: "service" });
      expect(view.state).toBe("unrecognised");
      expect(view.variant).toBe("unknown");
      expect(view.text).toBe("Pickup unknown");
      expect(view.words).toBe("The hub reported a pickup state that this version of the app does not know, so it cannot say when the change takes effect.");
      expect(view.action).toBe(false);
      for (const text of [view.text, view.name, view.words]) expect(text).not.toContain(bad);
    }
  });
});

describe("the counts to show in a summary", () => {
  it("lists each state once, most urgent first, with the services that are in it", () => {
    expect(
      countsToShow(
        [
          ["needs_notify_or_restart", 2],
          ["needs_config_server_restart", 1],
          ["live_within_ttl", 4],
        ],
        "all",
      ),
    ).toEqual([
      ["needs_config_server_restart", 1],
      ["needs_notify_or_restart", 2],
      ["live_within_ttl", 4],
    ]);
  });

  it("leaves out the services that pick the change up by themselves when only the ones that need a person are asked for", () => {
    expect(
      countsToShow(
        [
          ["live_within_ttl", 4],
          ["needs_notify_or_restart", 1],
        ],
        "actionable",
      ),
    ).toEqual([["needs_notify_or_restart", 1]]);
  });

  it("adds a state that is listed twice, and drops a count that is not a whole positive number", () => {
    expect(
      countsToShow(
        [
          ["needs_notify_or_restart", 1],
          ["needs_notify_or_restart", 2],
          ["needs_config_server_restart", -1],
          ["live_within_ttl", Number.NaN],
          ["live_within_ttl", 1.5],
        ],
        "all",
      ),
    ).toEqual([["needs_notify_or_restart", 3]]);
  });

  it("drops live (the contract lists only the states that are not live) and a state it does not know", () => {
    expect(
      countsToShow(
        [
          ["live", 9],
          ["bogus" as PickupState, 2],
        ],
        "all",
      ),
    ).toEqual([]);
  });

  it("is empty for an absent list, which is what the hub sends when nothing waits", () => {
    expect(countsToShow(undefined, "all")).toEqual([]);
    expect(countsToShow([], "actionable")).toEqual([]);
  });
});

describe("how long a client keeps its cache", () => {
  it.each<[number, string]>([
    [0, "0 s"],
    [45, "45 s"],
    [60, "1 min"],
    [300, "5 min"],
    [1200, "20 min"],
    [90, "1 min 30 s"],
    [3600, "1 h"],
    [7200, "2 h"],
    [5400, "1 h 30 min"],
    [90_000, "25 h"],
  ])("%i seconds is %s", (secs, text) => {
    expect(formatTtl(secs)).toBe(text);
  });

  it.each([Number.NaN, -1, 1.5, Number.POSITIVE_INFINITY, "300", null, undefined])("is nothing for %j", (value) => {
    expect(formatTtl(value)).toBeNull();
  });
});

describe("the notifications flag, as the hub reported it (Q35)", () => {
  it("says on and off and that the hub said so, and nothing about what the flag does", () => {
    expect(notificationsText(true)).toBe("Notifications: on (as reported by the hub)");
    expect(notificationsText(false)).toBe("Notifications: off (as reported by the hub)");
  });

  it("says that the hub did not report it when it is not a yes or a no", () => {
    for (const bad of [null, undefined, "true", 1, 0, {}]) expect(notificationsText(bad)).toBe("Notifications: not reported by the hub");
  });

  it("makes no claim about listening, subscribing, receiving or working", () => {
    for (const value of [true, false, null]) expect(notificationsText(value)).not.toMatch(/subscrib|listen|receiv|work|enabled|disabled|will|can/i);
  });
});

describe("the day of the week does not matter", () => {
  it("reads the same one day on", () => {
    const next = liveByView(Date.UTC(2026, 9, 17, 14, 32) + DAY, NOON + DAY);
    expect(next.text).toBe("14:32 UTC");
  });
});

describe("a time that can be shown", () => {
  it("is a number in the range the engine can show", () => {
    expect(isShownTime(0)).toBe(true);
    expect(isShownTime(NOON)).toBe(true);
    expect(isShownTime(253_402_300_799_999)).toBe(true);
  });

  it.each([null, undefined, Number.NaN, Number.POSITIVE_INFINITY, -1, 253_402_300_800_000, "1760000000000", {}])("is not %j", (value) => {
    expect(isShownTime(value)).toBe(false);
  });
});
