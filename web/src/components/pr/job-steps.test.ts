import { describe, expect, it } from "vitest";
import { PrJobState } from "@/api/generated/model";
import { isTerminal, jobSteps, phaseOf } from "./job-steps";

// A job's progress is what the server reports and nothing else: its state. The four steps are the state's, a queued job has not
// begun, and a failed job does not say how far it got, so it shows no step as done.

describe("phaseOf", () => {
  it.each([
    [PrJobState.pending, "queued"],
    [PrJobState.preparing, "running"],
    [PrJobState.committing, "running"],
    [PrJobState.opening, "running"],
    [PrJobState.completed, "completed"],
    [PrJobState.failed, "failed"],
  ] as const)("%s is %s", (state, phase) => {
    expect(phaseOf(state)).toBe(phase);
  });

  it("covers every state the contract has (a new one fails here, not on screen)", () => {
    for (const state of Object.values(PrJobState)) expect(["queued", "running", "completed", "failed"]).toContain(phaseOf(state));
  });
});

describe("jobSteps", () => {
  const status = (state: Parameters<typeof jobSteps>[0]) => jobSteps(state).map((s) => s.status);

  it("starts none for a queued job", () => {
    expect(status(PrJobState.pending)).toEqual(["waiting", "waiting", "waiting", "waiting"]);
  });

  it("moves one step at a time with the state the server reports", () => {
    expect(status(PrJobState.preparing)).toEqual(["active", "waiting", "waiting", "waiting"]);
    expect(status(PrJobState.committing)).toEqual(["done", "active", "waiting", "waiting"]);
    expect(status(PrJobState.opening)).toEqual(["done", "done", "active", "waiting"]);
  });

  it("finishes all four only for a completed job", () => {
    expect(status(PrJobState.completed)).toEqual(["done", "done", "done", "done"]);
    for (const state of [PrJobState.pending, PrJobState.preparing, PrJobState.committing, PrJobState.opening, PrJobState.failed]) {
      expect(status(state)).not.toEqual(["done", "done", "done", "done"]);
    }
  });

  it("shows no steps for a failed job, which does not say how far it got", () => {
    expect(jobSteps(PrJobState.failed)).toEqual([]);
  });

  it("names the steps as the design does", () => {
    expect(jobSteps(PrJobState.completed).map((s) => s.label)).toEqual(["Preparing", "Committing", "Opening pull request", "Done"]);
  });
});

describe("isTerminal", () => {
  it("is true for completed and failed, and for nothing else", () => {
    expect(Object.values(PrJobState).filter(isTerminal).sort()).toEqual(["completed", "failed"]);
  });
});
