import { describe, expect, it } from "vitest";
import { act, renderHook } from "@testing-library/react";
import { QueryClientProvider, type QueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { ApiProblem } from "@/api/problem";
import { createQueryClient } from "./client";
import { useSecretMutation } from "./secret-mutation";

const SECRET = "ghp_SECRETVALUE0123456789abcdefghijkl";

function setup() {
  const client = createQueryClient();
  const wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { client, wrapper };
}

function everythingTheClientHolds(client: QueryClient): string {
  const seen = new WeakSet();
  const dump = (value: unknown, depth = 0): string => {
    if (depth > 8 || typeof value !== "object" || value === null) return String(value);
    if (seen.has(value)) return "";
    seen.add(value);
    return Object.entries(value as Record<string, unknown>).map(([k, v]) => `${k}:${dump(v, depth + 1)}`).join("|");
  };
  return [
    ...client.getMutationCache().getAll().map((m) => dump(m.state) + dump(m.options.mutationKey)),
    ...client.getQueryCache().getAll().map((q) => dump(q.state) + dump(q.queryKey)),
  ].join("#");
}

describe("useSecretMutation", () => {
  it("returns the secret to the caller and leaves none in the query or mutation cache after success", async () => {
    const { client, wrapper } = setup();
    const { result, unmount } = renderHook(() => useSecretMutation((vars: { name: string }) => Promise.resolve({ token: SECRET, name: vars.name })), { wrapper });
    let value: { token: string; name: string } | undefined;
    await act(async () => {
      value = await result.current.run({ name: "laptop" });
    });
    expect(value?.token).toBe(SECRET);
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    unmount();
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });

  it("leaves no secret that was sent either, after a failure", async () => {
    const { client, wrapper } = setup();
    const { result } = renderHook(() => useSecretMutation((vars: { token: string }) => Promise.reject(new ApiProblem({ status: 422, kind: "token_rejected", title: `Token rejected (${vars.token.length} characters)` }))), { wrapper });
    await act(async () => {
      await expect(result.current.run({ token: SECRET })).rejects.toMatchObject({ kind: "token_rejected" });
    });
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });

  it("is never retried: a failing mutation function runs once", async () => {
    const { wrapper } = setup();
    let calls = 0;
    const { result } = renderHook(
      () =>
        useSecretMutation(() => {
          calls += 1;
          return Promise.reject(new ApiProblem({ status: 0, kind: "network", title: "Could not reach the server" }));
        }),
      { wrapper },
    );
    await act(async () => {
      await result.current.run(undefined).catch(() => undefined);
    });
    expect(calls).toBe(1);
  });

  it("drops an in-flight mutation's state on unmount", async () => {
    const { client, wrapper } = setup();
    let release: (v: { token: string }) => void = () => undefined;
    let started: () => void = () => undefined;
    const running = new Promise<void>((resolve) => (started = resolve));
    const { result, unmount } = renderHook(
      () =>
        useSecretMutation(
          () =>
            new Promise<{ token: string }>((resolve) => {
              release = resolve;
              started();
            }),
        ),
      { wrapper },
    );
    let pending: Promise<unknown> = Promise.resolve();
    act(() => {
      pending = result.current.run(undefined as never).catch(() => undefined);
    });
    await running;
    unmount();
    await act(async () => {
      release({ token: SECRET });
      await pending;
    });
    expect(everythingTheClientHolds(client)).not.toContain(SECRET);
    expect(client.getMutationCache().getAll()).toHaveLength(0);
  });
});
