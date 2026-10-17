import { readFileSync } from "node:fs";
import { MutationObserver, QueryClient, QueryObserver } from "@tanstack/react-query";
import { describe, expect, expectTypeOf, it, vi } from "vitest";
import { parse } from "yaml";
import { ApiProblem } from "@/api/problem";
import type { AutoNotify } from "@/api/generated/model";
import { adminKeys, swimlaneKeys } from "./keys";
import { SECRET_OPERATIONS } from "./invalidate";
import { isOptimisticOperation, OPTIMISTIC, optimisticWriteMutation } from "./optimistic";
import type { MutatingOperationId } from "./invalidate";

// The one place that may touch the cache while a write is in flight. It cancels what is loading for the affected keys,
// remembers what they held, shows the expected answer, puts the old answer back if the write fails, and ALWAYS refetches
// the affected keys when the write settles, so the screen ends up showing the server's answer whatever happened.
//
// It is for a closed list of operations. The contract decides who is on it: a write that can answer 202 ("sent for approval"),
// carries an expected hash or If-Match (409), is secret-bearing or reveals a value is never optimistic, because the optimistic
// value would claim a result the server may refuse or has not decided.

const spec = parse(readFileSync(new URL("../../../../api/openapi.yaml", import.meta.url), "utf8")) as {
  paths: Record<string, Record<string, { operationId?: string; responses?: Record<string, unknown>; parameters?: Array<{ name?: string; $ref?: string }>; requestBody?: unknown }>>;
  components: { schemas: Record<string, { properties?: Record<string, unknown>; allOf?: unknown[] }>; parameters?: Record<string, { name?: string }> };
};
const operations = Object.entries(spec.paths).flatMap(([path, item]) =>
  Object.entries(item)
    .filter(([method]) => ["get", "post", "put", "patch", "delete"].includes(method))
    .map(([method, op]) => ({ id: op.operationId ?? "", method: method.toUpperCase(), path, codes: Object.keys(op.responses ?? {}), op })),
);

describe("who may be optimistic", () => {
  it("is a closed list: one operation, the auto-notify switch of a swimlane, and no other", () => {
    expect(Object.keys(OPTIMISTIC)).toEqual(["adminPutAutoNotify"]);
    expect(isOptimisticOperation("adminPutAutoNotify")).toBe(true);
    for (const id of ["editFile", "restartService", "putGithubToken", "revealValue", "adminUpdateUser", "approveProposal", "createMcpToken", "deleteFile"]) expect(isOptimisticOperation(id), id).toBe(false);
  });

  it("only names operations of the contract (the registry cannot hold a name that is not a mutating operation)", () => {
    expectTypeOf<keyof typeof OPTIMISTIC>().toExtend<MutatingOperationId>();
  });

  it("says in its own words why each one is safe", () => {
    for (const [id, entry] of Object.entries(OPTIMISTIC)) expect(entry.why.length, id).toBeGreaterThan(80);
  });

  it("holds only operations the CONTRACT shows to be honest: PUT, no 202, no 409, no 412, no 423, no expected hash or If-Match, not secret, not a reveal", () => {
    for (const id of Object.keys(OPTIMISTIC)) {
      const found = operations.find((o) => o.id === id);
      expect(found, `${id} is an operation of the contract`).toBeDefined();
      if (!found) continue;
      expect(found.method, `${id} is idempotent (a PUT with the whole value)`).toBe("PUT");
      for (const code of ["202", "409", "412", "423"]) expect(found.codes, `${id} can answer ${code}`).not.toContain(code);
      const names = (found.op.parameters ?? []).map((p) => p.name ?? spec.components.parameters?.[(p.$ref ?? "").split("/").pop() ?? ""]?.name ?? "");
      expect(names.map((n) => n.toLowerCase()), `${id} takes an expected hash or If-Match`).not.toEqual(expect.arrayContaining(["if-match"]));
      expect(JSON.stringify(found.op.requestBody ?? {}), `${id}'s body carries an expected hash`).not.toMatch(/Expected|expected/);
      expect((SECRET_OPERATIONS as readonly string[]).includes(id), `${id} is secret-bearing`).toBe(false);
      expect(id, `${id} reveals`).not.toMatch(/reveal/i);
    }
  });

  it("would catch a risky operation: every write that can answer 202, 409, 412 or 423 is refused by the same check", () => {
    const risky = operations.filter((o) => o.method !== "GET" && o.codes.some((c) => ["202", "409", "412", "423"].includes(c))).map((o) => o.id);
    expect(risky.length).toBeGreaterThan(10);
    for (const id of risky) expect(isOptimisticOperation(id), id).toBe(false);
  });
});

// ---- the helper ----

type Vars = { swimlane: string; body: AutoNotify };
const key = (swimlane: string) => adminKeys.list("auto-notify", { id: swimlane });

function setup(options: { cached?: AutoNotify | undefined; run: (vars: Vars) => Promise<AutoNotify> }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const server: AutoNotify = { enabled: false };
  const read = vi.fn(() => Promise.resolve({ ...server }));
  if (options.cached !== undefined) client.setQueryData(key("sit1"), options.cached);
  const observer = new QueryObserver(client, { queryKey: key("sit1"), queryFn: read, staleTime: Infinity, enabled: options.cached !== undefined });
  const seen: Array<AutoNotify | undefined> = [];
  const off = observer.subscribe((result) => seen.push(result.data));
  const mutation = new MutationObserver(
    client,
    optimisticWriteMutation<"adminPutAutoNotify", Vars, AutoNotify>(client, {
      operation: "adminPutAutoNotify",
      run: options.run,
      context: (vars) => ({ swimlane: vars.swimlane }),
    }),
  );
  return { client, read, server, seen, mutation, off };
}

describe("optimisticWriteMutation", () => {
  it("shows the expected answer before the write has answered", async () => {
    let answer: (value: AutoNotify) => void = () => undefined;
    const t = setup({ cached: { enabled: false }, run: () => new Promise((resolve) => (answer = resolve)) });
    const done = t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } });
    await vi.waitFor(() => expect(t.client.getQueryData(key("sit1"))).toEqual({ enabled: true }));
    answer({ enabled: true });
    await done;
    t.off();
    t.client.clear();
  });

  it("cancels a request that is loading for the key, so its late answer cannot overwrite the expected one", async () => {
    let finish: (value: AutoNotify) => void = () => undefined;
    const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
    client.setQueryData(key("sit1"), { enabled: false });
    const slow = vi.fn(() => new Promise<AutoNotify>((resolve) => (finish = resolve)));
    const observer = new QueryObserver(client, { queryKey: key("sit1"), queryFn: slow, staleTime: 0 });
    const off = observer.subscribe(() => undefined);
    await vi.waitFor(() => expect(slow).toHaveBeenCalled());
    const mutation = new MutationObserver(client, optimisticWriteMutation<"adminPutAutoNotify", Vars, AutoNotify>(client, { operation: "adminPutAutoNotify", run: () => new Promise(() => undefined), context: (v) => ({ swimlane: v.swimlane }) }));
    void mutation.mutate({ swimlane: "sit1", body: { enabled: true } }).catch(() => undefined);
    await vi.waitFor(() => expect(client.getQueryData(key("sit1"))).toEqual({ enabled: true }));
    finish({ enabled: false }); // the stale answer arrives late
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(client.getQueryData(key("sit1"))).toEqual({ enabled: true });
    off();
    client.clear();
  });

  it("on success, refetches the affected keys: the screen ends on the server's answer, not on the guess", async () => {
    const t = setup({ cached: { enabled: false }, run: () => Promise.resolve({ enabled: true }) });
    t.server.enabled = true;
    await t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } });
    expect(t.read).toHaveBeenCalledTimes(1);
    expect(t.client.getQueryData(key("sit1"))).toEqual({ enabled: true });
    t.off();
    t.client.clear();
  });

  it("on failure, puts the old answer back, and refetches anyway", async () => {
    const t = setup({ cached: { enabled: false }, run: () => Promise.reject(new ApiProblem({ status: 500, kind: "internal", title: "t" })) });
    await expect(t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } })).rejects.toBeInstanceOf(ApiProblem);
    expect(t.seen).toContainEqual({ enabled: true }); // it was shown...
    expect(t.read).toHaveBeenCalledTimes(1); // ...and the screen asked the server again
    expect(t.client.getQueryData(key("sit1"))).toEqual({ enabled: false }); // ...and ends on the truth
    t.off();
    t.client.clear();
  });

  it("refetches even after a refusal that changed nothing (400, 422): the helper always invalidates on settle", async () => {
    for (const status of [400, 403, 422]) {
      const t = setup({ cached: { enabled: false }, run: () => Promise.reject(new ApiProblem({ status, kind: "x", title: "t" })) });
      await expect(t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } })).rejects.toBeInstanceOf(ApiProblem);
      expect(t.read, String(status)).toHaveBeenCalledTimes(1);
      expect(t.client.getQueryData(key("sit1"))).toEqual({ enabled: false });
      t.off();
      t.client.clear();
    }
  });

  it("invents nothing when the key holds no data: no entry is created from a guess", async () => {
    const t = setup({ cached: undefined, run: () => Promise.resolve({ enabled: true }) });
    await t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } });
    expect(t.client.getQueryData(key("sit1"))).toBeUndefined();
    t.off();
    t.client.clear();
  });

  it("leaves every other key alone: another swimlane's switch and the swimlane's own summary are not patched", async () => {
    const t = setup({ cached: { enabled: false }, run: () => Promise.resolve({ enabled: true }) });
    t.client.setQueryData(key("sit2"), { enabled: false });
    t.client.setQueryData(swimlaneKeys.detail("sit1"), { id: "sit1" });
    await t.mutation.mutate({ swimlane: "sit1", body: { enabled: true } });
    expect(t.client.getQueryData(key("sit2"))).toEqual({ enabled: false });
    expect(t.client.getQueryData(swimlaneKeys.detail("sit1"))).toEqual({ id: "sit1" });
    // What the write table says it changed is refetched or marked stale.
    expect(t.client.getQueryState(swimlaneKeys.detail("sit1"))?.isInvalidated).toBe(true);
    t.off();
    t.client.clear();
  });

  it("is never retried by itself, and keeps nothing once nobody watches it", () => {
    const client = new QueryClient();
    const options = optimisticWriteMutation<"adminPutAutoNotify", Vars, AutoNotify>(client, { operation: "adminPutAutoNotify", run: () => Promise.resolve({ enabled: true }), context: (v) => ({ swimlane: v.swimlane }) });
    expect(options.retry).toBe(false);
    expect(options.gcTime).toBe(0);
    client.clear();
  });
});
