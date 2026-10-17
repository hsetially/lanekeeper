import { MutationObserver, QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, it, vi } from "vitest";
import { ApiProblem } from "@/api/problem";
import { fileKeys, proposalKeys } from "./keys";
import { writeMutation } from "./write";

// A plain write (no optimistic value, no secret): it runs, and what it changed is invalidated by the table of invalidate.ts.

function setup() {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false }, queries: { retry: false } } });
  const head = vi.fn(() => Promise.resolve({ hash: "new" }));
  client.setQueryData(fileKeys.head("sit1", "a.yml"), { hash: "old" });
  new QueryObserver(client, { queryKey: fileKeys.head("sit1", "a.yml"), queryFn: head, staleTime: Infinity }).subscribe(() => undefined);
  const queue = vi.fn(() => Promise.resolve([]));
  client.setQueryData(proposalKeys.lists(), []);
  new QueryObserver(client, { queryKey: proposalKeys.lists(), queryFn: queue, staleTime: Infinity }).subscribe(() => undefined);
  return { client, head, queue };
}

type Vars = { swimlane: string; path: string };
const spec = (run: () => Promise<{ outcome: "applied" | "proposal_created" }>) => ({
  operation: "editFile" as const,
  run,
  context: (vars: Vars, result: { outcome: "applied" | "proposal_created" } | undefined) => ({ ...vars, ...(result ? { outcome: result.outcome } : {}) }),
});

describe("writeMutation", () => {
  it("runs the write, then refetches what it changed, and is done only when the screen is up to date", async () => {
    const { client, head, queue } = setup();
    const observer = new MutationObserver(client, writeMutation<"editFile", Vars, { outcome: "applied" | "proposal_created" }>(client, spec(() => Promise.resolve({ outcome: "applied" }))));
    await observer.mutate({ swimlane: "sit1", path: "a.yml" });
    expect(head).toHaveBeenCalledTimes(1);
    expect(queue).not.toHaveBeenCalled();
    client.clear();
  });

  it("when the write was sent for approval, refetches the queue and not the file", async () => {
    const { client, head, queue } = setup();
    const observer = new MutationObserver(client, writeMutation<"editFile", Vars, { outcome: "applied" | "proposal_created" }>(client, spec(() => Promise.resolve({ outcome: "proposal_created" }))));
    await observer.mutate({ swimlane: "sit1", path: "a.yml" });
    expect(head).not.toHaveBeenCalled();
    expect(queue).toHaveBeenCalledTimes(1);
    client.clear();
  });

  it.each([409, 423, 503])("after a %i the cache is out of date, so it refetches (outcome unknown, so as for an applied write)", async (status) => {
    const { client, head } = setup();
    const observer = new MutationObserver(client, writeMutation<"editFile", Vars, { outcome: "applied" | "proposal_created" }>(client, spec(() => Promise.reject(new ApiProblem({ status, kind: "x", title: "t" })))));
    await expect(observer.mutate({ swimlane: "sit1", path: "a.yml" })).rejects.toBeInstanceOf(ApiProblem);
    expect(head).toHaveBeenCalledTimes(1);
    client.clear();
  });

  it.each([400, 403, 404, 422, 429])("after a %i nothing changed, so it refetches nothing", async (status) => {
    const { client, head } = setup();
    const observer = new MutationObserver(client, writeMutation<"editFile", Vars, { outcome: "applied" | "proposal_created" }>(client, spec(() => Promise.reject(new ApiProblem({ status, kind: "x", title: "t" })))));
    await expect(observer.mutate({ swimlane: "sit1", path: "a.yml" })).rejects.toBeInstanceOf(ApiProblem);
    expect(head).not.toHaveBeenCalled();
    client.clear();
  });

  it("never retries a write by itself, and keeps nothing of it once nobody watches it (a write carries file content)", () => {
    const { client } = setup();
    const options = writeMutation<"editFile", Vars, { outcome: "applied" | "proposal_created" }>(client, spec(() => Promise.resolve({ outcome: "applied" })));
    expect(options.retry).toBe(false);
    expect(options.gcTime).toBe(0);
    expect(options.mutationKey).toEqual(["write", "editFile"]);
    expect(options).not.toHaveProperty("onMutate");
    client.clear();
  });
});
