import { describe, expect, it } from "vitest";
import { agentView, offlineSince } from "./agent";

// The contract says an agent is connected, disconnected or never seen. The design draws three dots (online, degraded, offline) and the
// words Online, Degraded and "Offline · 14m". The mapping is written once, here. The contract has no "degraded", so it is never drawn.

describe("agentView", () => {
  it("maps a connected agent to the online dot", () => {
    expect(agentView("connected")).toEqual({ state: "online", label: "Online", connected: true });
  });

  it("maps a disconnected agent to the offline dot", () => {
    expect(agentView("disconnected")).toEqual({ state: "offline", label: "Offline", connected: false });
  });

  it("maps an agent that never reported to the offline dot and says so", () => {
    expect(agentView("never_seen")).toEqual({ state: "offline", label: "Never seen", connected: false });
  });

  it("treats an agent state it does not know as offline, never as online", () => {
    expect(agentView("something-new" as never).connected).toBe(false);
  });
});

describe("offlineSince", () => {
  const NOW = 1_760_000_000_000;

  it("is the time of the last scan for a disconnected agent, because that is when it last reported", () => {
    expect(offlineSince("disconnected", NOW - 14 * 60_000)).toBe(NOW - 14 * 60_000);
  });

  it("is nothing for a connected agent and for one that never reported", () => {
    expect(offlineSince("connected", NOW)).toBeNull();
    expect(offlineSince("never_seen", null)).toBeNull();
  });

  it("is nothing for a disconnected agent whose last scan is unknown", () => {
    expect(offlineSince("disconnected", null)).toBeNull();
  });
});
