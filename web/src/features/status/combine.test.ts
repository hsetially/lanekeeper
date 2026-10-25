import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import type { QueryState } from "@/lib/query/use-query-state";
import { combine } from "./combine";

// A screen that needs two answers (the flags and the swimlanes) shows one state for them: the first failure, else loading while
// either is, else empty or ready. What is stale is the union of what is stale.

const state = <T,>(over: Partial<QueryState<T>> & Pick<QueryState<T>, "state">): QueryState<T> => ({ data: undefined, error: null, retryable: false, refreshing: false, stale: null, refetch: vi.fn(), ...over });
const problem = new ApiProblem({ status: 500, kind: "internal", title: "Internal error", requestId: "r1" });

describe("combine", () => {
  it("is ready with both answers", () => {
    const out = combine(state({ state: "ready", data: 1 }), state({ state: "ready", data: "a" }));
    expect(out).toMatchObject({ state: "ready", data: [1, "a"] });
  });

  it("is empty when the first is empty and the other is ready", () => {
    expect(combine(state({ state: "empty", data: 0 }), state({ state: "ready", data: "a" })).state).toBe("empty");
  });

  it("is loading while either is", () => {
    expect(combine(state({ state: "loading" }), state({ state: "ready", data: "a" })).state).toBe("loading");
    expect(combine(state({ state: "ready", data: 1 }), state({ state: "loading" })).state).toBe("loading");
  });

  it("is the first error, with its problem and whether trying again can help", () => {
    const out = combine(state({ state: "ready", data: 1 }), state({ state: "error", error: problem, retryable: true }));
    expect(out).toMatchObject({ state: "error", error: problem, retryable: true });
    const first = combine(state({ state: "error", error: problem }), state({ state: "error", error: new ApiProblem({ status: 403, kind: "forbidden", title: "No" }) }));
    expect(first.error).toBe(problem);
  });

  it("refetches only what failed, or both when both are fine", () => {
    const a = state<number>({ state: "ready", data: 1 });
    const b = state<string>({ state: "error", error: problem });
    combine(a, b).refetch();
    expect(a.refetch).not.toHaveBeenCalled();
    expect(b.refetch).toHaveBeenCalledTimes(1);
    const c = state<number>({ state: "ready", data: 1 });
    const d = state<string>({ state: "ready", data: "a" });
    combine(c, d).refetch();
    expect(c.refetch).toHaveBeenCalledTimes(1);
    expect(d.refetch).toHaveBeenCalledTimes(1);
  });

  it("joins what is stale, and is refreshing when either is", () => {
    const out = combine(
      state({ state: "ready", data: 1, stale: { causes: ["age"], asOf: 200 }, refreshing: true }),
      state({ state: "ready", data: "a", stale: { causes: ["live-paused", "age"], asOf: 100 } }),
    );
    expect(out.refreshing).toBe(true);
    expect(out.stale).toEqual({ causes: ["age", "live-paused"], asOf: 100 });
    expect(combine(state({ state: "ready", data: 1 }), state({ state: "ready", data: "a" })).stale).toBeNull();
  });
});
