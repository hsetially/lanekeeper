import { describe, expect, it } from "vitest";
import { ApiProblem } from "@/api/problem";
import { retryDelayMs, shouldRetry } from "@/api/retry";
import { createQueryClient } from "./client";

const problem = (status: number, kind = "unknown") => new ApiProblem({ status, kind, title: "x" });

describe("shouldRetry", () => {
  it.each([400, 401, 403, 404, 409, 422, 423, 429])("never retries a %i, the screens handle it", (status) => {
    for (let failures = 0; failures < 5; failures++) expect(shouldRetry(failures, problem(status))).toBe(false);
  });

  it.each([502, 503, 504])("retries a %i twice and no more", (status) => {
    expect(shouldRetry(0, problem(status))).toBe(true);
    expect(shouldRetry(1, problem(status))).toBe(true);
    expect(shouldRetry(2, problem(status))).toBe(false);
  });

  it("does not retry a 500: the server said something went wrong, not that it was busy", () => {
    expect(shouldRetry(0, problem(500))).toBe(false);
  });

  it("retries a network error twice", () => {
    const network = problem(0, "network");
    expect(shouldRetry(0, network)).toBe(true);
    expect(shouldRetry(1, network)).toBe(true);
    expect(shouldRetry(2, network)).toBe(false);
  });

  it("does not retry a timeout more than twice either", () => {
    expect(shouldRetry(2, problem(0, "timeout"))).toBe(false);
  });

  it("never retries something that is not an API problem (a bug in our code)", () => {
    expect(shouldRetry(0, new TypeError("x is not a function"))).toBe(false);
    expect(shouldRetry(0, "boom")).toBe(false);
  });
});

describe("retryDelayMs", () => {
  it("is full jitter: between zero and the capped exponential, never above the cap", () => {
    for (let attempt = 0; attempt < 12; attempt++) {
      expect(retryDelayMs(attempt, () => 0)).toBe(0);
      const top = retryDelayMs(attempt, () => 0.999999);
      expect(top).toBeLessThanOrEqual(8000);
      expect(top).toBeGreaterThanOrEqual(0);
    }
    expect(retryDelayMs(0, () => 0.999999)).toBeLessThanOrEqual(500);
    expect(retryDelayMs(1, () => 0.999999)).toBeLessThanOrEqual(1000);
    expect(retryDelayMs(10, () => 0.999999)).toBeLessThanOrEqual(8000);
  });
});

describe("createQueryClient", () => {
  it("uses the retry policy for queries and never retries a mutation", () => {
    const client = createQueryClient();
    const defaults = client.getDefaultOptions();
    expect(defaults.queries?.retry).toBe(shouldRetry);
    expect(defaults.mutations?.retry).toBe(false);
  });

  it("does not refetch on window focus, and bounds how long unused data is kept", () => {
    const defaults = createQueryClient().getDefaultOptions();
    expect(defaults.queries?.refetchOnWindowFocus).toBe(false);
    expect(typeof defaults.queries?.gcTime).toBe("number");
    expect(defaults.queries?.gcTime).toBeLessThanOrEqual(10 * 60 * 1000);
  });
});
