import { describe, expect, it } from "vitest";
import { serviceWriteGate } from "./gates";

// Who may press a button that acts on a swimlane's services, and why not: the permission first (waiting does not fix it), then
// the live connection (the shell's answer, folded into `base`), then the agent (a notification and a restart both go through it).

const allowed = { allowed: true, reason: null };

describe("serviceWriteGate", () => {
  it("allows an allowed person when the agent is connected", () => {
    expect(serviceWriteGate(allowed, "connected")).toEqual(allowed);
  });

  it("keeps the reason of the role, or of the paused live updates, before the agent's", () => {
    const role = { allowed: false, reason: "Needs the Operator role. Ask an Admin to grant it." };
    expect(serviceWriteGate(role, "disconnected")).toBe(role);
    const offline = { allowed: false, reason: "Unavailable offline" };
    expect(serviceWriteGate(offline, "never_seen")).toBe(offline);
  });

  it.each(["disconnected", "never_seen"] as const)("is off while the agent is %s, and says so", (agent) => {
    expect(serviceWriteGate(allowed, agent)).toEqual({ allowed: false, reason: "Unavailable while the agent is offline" });
  });
});
