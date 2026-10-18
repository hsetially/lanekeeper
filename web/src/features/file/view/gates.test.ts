import { describe, expect, it } from "vitest";
import { fileGates, type GateInput } from "./gates";

// Who may press which button on the file view, and why not. A button is never hidden for a reason a person can fix: it is
// disabled with the reason (the permission first, because waiting does not fix it). The server enforces all of it anyway.

const open: GateInput = { base: { allowed: true, reason: null }, agent: "connected", lane: "sitb", masked: false, fileClass: "structured", withheld: false, tokenExpired: false, needsGithubToken: false };
const reasons = (input: Partial<GateInput>) => fileGates({ ...open, ...input });

describe("fileGates", () => {
  it("lets an Editor with a connected agent do everything", () => {
    const g = reasons({});
    expect([g.edit, g.upload, g.delete, g.revert, g.pr].every((x) => x.allowed)).toBe(true);
  });

  it("gives the permission reason first, and the same one for every action", () => {
    const reason = "Needs the Editor role. Ask an Admin to grant it.";
    const g = reasons({ base: { allowed: false, reason }, agent: "disconnected", masked: true });
    for (const gate of [g.edit, g.upload, g.delete, g.revert, g.pr]) expect(gate).toEqual({ allowed: false, reason });
  });

  it("turns off what writes to the swimlane while its agent is offline, and not the pull request", () => {
    const g = reasons({ agent: "disconnected" });
    for (const gate of [g.edit, g.upload, g.delete, g.revert]) expect(gate).toEqual({ allowed: false, reason: "Unavailable while the agent is offline" });
    expect(g.pr.allowed).toBe(true);
  });

  it("treats an agent that never reported as offline too", () => {
    expect(reasons({ agent: "never_seen" }).edit.allowed).toBe(false);
  });

  it("waits for the swimlane before it allows a write", () => {
    expect(reasons({ agent: undefined }).edit.allowed).toBe(false);
  });

  it("will not edit a masked view: saving it would write the marker over the value it hides", () => {
    const g = reasons({ masked: true });
    expect(g.edit.allowed).toBe(false);
    expect(g.edit.reason).toMatch(/masked/i);
    expect(g.upload.allowed).toBe(true);
  });

  it("will not edit a binary file, but can replace it", () => {
    const g = reasons({ fileClass: "binary" });
    expect(g.edit.allowed).toBe(false);
    expect(g.edit.reason).toMatch(/binary/i);
    expect(g.upload.allowed).toBe(true);
  });

  it("has no edit action on a file whose content is withheld", () => {
    const g = reasons({ fileClass: "denied", withheld: true });
    for (const gate of [g.edit, g.upload, g.delete, g.revert, g.pr]) expect(gate.allowed).toBe(false);
  });

  it("turns Git-dependent work off when the token expired, and says where to renew it", () => {
    const g = reasons({ tokenExpired: true });
    expect(g.pr).toEqual({ allowed: false, reason: "Your GitHub token expired. Update it in My GitHub token." });
    expect(g.edit.allowed).toBe(true);
  });

  it("asks for a token before the first pull request", () => {
    expect(reasons({ needsGithubToken: true }).pr.reason).toMatch(/GitHub token/);
  });
});
