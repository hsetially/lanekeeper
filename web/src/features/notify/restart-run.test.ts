import { describe, expect, it } from "vitest";
import type { ServiceRef } from "@/api/generated/model";
import { abandonRest, begin, describeStop, mark, summarise } from "./restart-run";

// "Restart all" sends one restart request per service, one at a time. These are the bookkeeping of that run: which request the
// run is on, what each answered, and what was never attempted because an earlier one was refused. Nothing here says a service
// restarted: a step is only ever what the hub answered to ONE request.

const svc = (name: string): ServiceRef => ({ namespace: "sitb", name });
const three = [svc("a"), svc("b"), svc("c")];

describe("begin", () => {
  it("lists every service as waiting, in the order the hub gave them", () => {
    expect(begin(three).map((s) => [s.service.name, s.state])).toEqual([["a", "waiting"], ["b", "waiting"], ["c", "waiting"]]);
  });
});

describe("mark", () => {
  it("changes one step and leaves the others and the original alone", () => {
    const steps = begin(three);
    const next = mark(steps, 1, "accepted");
    expect(next.map((s) => s.state)).toEqual(["waiting", "accepted", "waiting"]);
    expect(steps.map((s) => s.state)).toEqual(["waiting", "waiting", "waiting"]);
  });

  it("keeps the request a step was sent for approval as", () => {
    expect(mark(begin(three), 0, "sent-for-approval", 12)[0]).toEqual({ service: svc("a"), state: "sent-for-approval", request: 12 });
  });

  it("ignores a step that is not there", () => {
    expect(mark(begin(three), 7, "accepted").map((s) => s.state)).toEqual(["waiting", "waiting", "waiting"]);
  });
});

describe("abandonRest", () => {
  it("marks every step from the one after the refusal as not attempted, and nothing before it", () => {
    let steps = begin(three);
    steps = mark(steps, 0, "accepted");
    steps = mark(steps, 1, "failed");
    expect(abandonRest(steps, 2).map((s) => s.state)).toEqual(["accepted", "failed", "not-attempted"]);
  });

  it("does not turn a step that already answered into one that was not attempted", () => {
    const steps = mark(begin(three), 2, "accepted");
    expect(abandonRest(steps, 1).map((s) => s.state)).toEqual(["waiting", "not-attempted", "accepted"]);
  });
});

describe("summarise", () => {
  it("counts each answer, and says the run stopped early only when something was not attempted", () => {
    let steps = begin(three);
    steps = mark(steps, 0, "accepted");
    steps = mark(steps, 1, "failed");
    steps = abandonRest(steps, 2);
    expect(summarise(steps)).toEqual({ total: 3, accepted: 1, sentForApproval: 0, failed: 1, notAttempted: 1, stoppedEarly: true });
  });

  it("is not stopped early when every request was answered, even if some were sent for approval", () => {
    let steps = begin(three);
    steps = mark(steps, 0, "accepted");
    steps = mark(steps, 1, "sent-for-approval", 4);
    steps = mark(steps, 2, "sent-for-approval", 5);
    expect(summarise(steps)).toEqual({ total: 3, accepted: 1, sentForApproval: 2, failed: 0, notAttempted: 0, stoppedEarly: false });
  });
});

describe("describeStop", () => {
  const stop = (accepted: number, sent: number, notAttempted: number) => describeStop({ total: accepted + sent + 1 + notAttempted, accepted, sentForApproval: sent, failed: 1, notAttempted, stoppedEarly: notAttempted > 0 }, "sitb/b");

  it("names the service whose request the hub refused, and counts what came before and after", () => {
    expect(stop(1, 0, 1)).toBe("The hub refused the request for sitb/b. 1 earlier request was accepted and 1 was not sent.");
    expect(stop(2, 0, 3)).toBe("The hub refused the request for sitb/b. 2 earlier requests were accepted and 3 were not sent.");
  });

  it("says what was sent for approval before it", () => {
    expect(stop(1, 2, 1)).toBe("The hub refused the request for sitb/b. 1 earlier request was accepted, 2 were sent for approval, and 1 was not sent.");
  });

  it("says only the refusal when it was the first and the last request", () => {
    expect(stop(0, 0, 0)).toBe("The hub refused the request for sitb/b.");
  });

  it("says nothing was sent after the first request when it is refused", () => {
    expect(stop(0, 0, 2)).toBe("The hub refused the request for sitb/b. 2 requests were not sent.");
  });
});
