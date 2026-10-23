import { QueryObserver } from "@tanstack/react-query";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createQueryClient } from "@/lib/query/client";
import { POLLED_MS } from "@/lib/query/freshness";
import { isImmutableKey, keyRoot, swimlaneKeys } from "@/lib/query/keys";
import { invalidateTargets, matches } from "@/lib/query/targets";
import { SERVED_GC_MS, servedFileQuery } from "./queries";

// The query behind the "As served" view: one exact request, keyed by everything it depends on, never trusted for long. The call goes
// through the real generated client and the real fetch layer, to a fetch that records what it was asked.

type Call = { url: URL; init: RequestInit };
let calls: Call[];

beforeEach(() => {
  calls = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: string, init: RequestInit = {}) => {
      calls.push({ url: new URL(input, "http://hub.test"), init });
      return Promise.resolve(new Response("{}", { status: 200, headers: { "content-type": "application/json" } }));
    }),
  );
});
afterEach(() => {
  vi.unstubAllGlobals();
});

// `queryOptions` types its function as one of three things; the module's own tests call it the way TanStack does.
type Runnable = { queryFn: (context: never) => Promise<unknown> };
const run = (options: unknown, signal: AbortSignal = new AbortController().signal): Promise<unknown> => (options as Runnable).queryFn({ signal, queryKey: (options as { queryKey: unknown }).queryKey } as never);

const SELECTION = { file: "core/tx-infinity-core-sit7.yml", application: "tx-infinity-core", tenant: "sit7", channel: null };

describe("servedFileQuery", () => {
  it("reads getServedFile for the swimlane with the application, tenant and file, and the abort signal", async () => {
    const options = servedFileQuery("sit1", SELECTION);
    await run(options);
    expect(calls).toHaveLength(1);
    const call = calls[0];
    expect(call?.url.pathname).toBe("/api/v1/swimlanes/sit1/served");
    expect(Object.fromEntries(call?.url.searchParams ?? [])).toEqual({ application: "tx-infinity-core", tenant: "sit7", file: "core/tx-infinity-core-sit7.yml" });
    expect(call?.init.method).toBe("GET");
    expect(call?.init.signal).toBeInstanceOf(AbortSignal);
  });

  it("sends the channel only when there is one", async () => {
    const options = servedFileQuery("sit1", { ...SELECTION, channel: "beta" });
    await run(options);
    expect(calls[0]?.url.searchParams.get("channel")).toBe("beta");
    expect(servedFileQuery("sit1", SELECTION).queryKey).not.toEqual(options.queryKey);
  });

  it("is keyed by the key factory, by everything the answer depends on", () => {
    expect(servedFileQuery("sit1", SELECTION).queryKey).toEqual(swimlaneKeys.served("sit1", "tx-infinity-core", "sit7", null, "core/tx-infinity-core-sit7.yml"));
    const base = servedFileQuery("sit1", SELECTION).queryKey;
    for (const other of [
      servedFileQuery("sit2", SELECTION),
      servedFileQuery("sit1", { ...SELECTION, application: "other" }),
      servedFileQuery("sit1", { ...SELECTION, tenant: "sit9" }),
      servedFileQuery("sit1", { ...SELECTION, channel: "beta" }),
      servedFileQuery("sit1", { ...SELECTION, file: "core/other.yml" }),
    ]) {
      expect(other.queryKey).not.toEqual(base);
    }
  });

  it("has a root of its own: the events that refresh the swimlane do not restart an agent round trip", () => {
    const key = servedFileQuery("sit1", SELECTION).queryKey;
    expect(keyRoot(key)).toBe("served");
    expect(isImmutableKey(key)).toBe(false);
    // Neither the swimlane's summary nor its detail, which `file_observed` and `drift_changed` name, reaches it.
    expect(matches({ kind: "key", key: swimlaneKeys.detail("sit1") }, key, undefined)).toBe(false);
    expect(matches({ kind: "key", key: swimlaneKeys.all() }, key, undefined)).toBe(false);
  });

  it("is fresh for a minute and is dropped half a minute after the screen leaves: it is what the config-server says now", () => {
    const options = servedFileQuery("sit1", SELECTION);
    expect(options.staleTime).toBe(POLLED_MS);
    expect(options.gcTime).toBe(SERVED_GC_MS);
    expect(SERVED_GC_MS).toBeLessThanOrEqual(60_000);
  });

  it("is not read again by a resync: it is asked for, never pushed, so the first connection of the event stream does not cost a second round trip to the agent", async () => {
    // The live layer reads every active query again when the stream has no id to resume from (the first connection of a page), after
    // it was offline, and when the hub says `resync` (src/live/sse-client.ts). No event names this answer and it says by itself when it
    // may be old, so a resync must not ask the agent again: a page opened cold would otherwise make two round trips, and the first
    // would be cancelled when it was still out.
    let controlReads = 0;
    const client = createQueryClient();
    const served = new QueryObserver(client, servedFileQuery("sit1", SELECTION));
    const control = new QueryObserver(client, {
      queryKey: ["control", "sit1"],
      queryFn: () => {
        controlReads += 1;
        return Promise.resolve(controlReads);
      },
      staleTime: 60_000,
    });
    const stops = [served.subscribe(() => undefined), control.subscribe(() => undefined)];
    await vi.waitFor(() => expect(served.getCurrentResult().status).toBe("success"));
    await vi.waitFor(() => expect(control.getCurrentResult().status).toBe("success"));
    expect(calls).toHaveLength(1);
    expect(controlReads).toBe(1);
    await invalidateTargets(client, "everything");
    // The control was read again by the same resync, so the check is not vacuous; the answer from the config-server was not asked for again.
    expect(controlReads).toBe(2);
    expect(calls).toHaveLength(1);
    expect(client.getQueryState(servedFileQuery("sit1", SELECTION).queryKey)?.isInvalidated, "not even marked stale").toBe(false);
    for (const stop of stops) stop();
    client.clear();
  });

  it("asks nothing without a selection", async () => {
    const options = servedFileQuery("sit1", null);
    expect(options.enabled).toBe(false);
    expect(servedFileQuery("sit1", SELECTION).enabled).toBe(true);
    // Even if something forces a read (a refetch of a disabled query runs the function), no request goes out.
    await expect(run(options)).rejects.toThrow();
    expect(calls).toHaveLength(0);
  });

  it("is not tried again after a failure: each request is a rate-limited round trip to an agent, and the page has Ask again", async () => {
    // The app's default is two more tries after a 503 (src/api/retry.ts). This query overrides it: one request, then the error, and
    // the person decides whether to ask again. Run against a client with the real defaults, a 503 must reach the screen at once.
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string, init: RequestInit = {}) => {
        calls.push({ url: new URL(input, "http://hub.test"), init });
        return Promise.resolve(new Response(JSON.stringify({ type: "about:blank", title: "Unavailable", status: 503 }), { status: 503, headers: { "content-type": "application/problem+json" } }));
      }),
    );
    expect(servedFileQuery("sit1", SELECTION).retry).toBe(false);
    const client = createQueryClient();
    const observer = new QueryObserver(client, servedFileQuery("sit1", SELECTION));
    const unsubscribe = observer.subscribe(() => undefined);
    await vi.waitFor(() => expect(observer.getCurrentResult().status).toBe("error"), { timeout: 800 });
    expect(calls).toHaveLength(1);
    unsubscribe();
    client.clear();
  });
});
