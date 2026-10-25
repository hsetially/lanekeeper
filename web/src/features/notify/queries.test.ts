import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { plan, matches } from "@/live/invalidation";
import { swimlaneKeys, type QueryKey } from "@/lib/query/keys";
import { configServerRestartOfferQuery, restartOfferQuery } from "./queries";

// The two offers a person confirms before a restart: what the config-server restart would take away, and what a restart of the
// services that read a file would restart. Both are an Operator's read, answered `no-store`, and both decide a destructive
// confirmation, so neither is ever shown from an old answer: they are asked again each time they are wanted, and nothing of
// them is kept once nobody is looking.

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

// `queryOptions` types its function as one of three things; this module's tests call it the way TanStack does.
type Runnable = { queryFn: (context: never) => Promise<unknown>; queryKey: unknown };
const run = (options: unknown): Promise<unknown> => (options as Runnable).queryFn({ signal: new AbortController().signal, queryKey: (options as Runnable).queryKey } as never);

describe("configServerRestartOfferQuery", () => {
  it("is keyed by the swimlane's key factory and reads the operation of the contract with the abort signal", async () => {
    const options = configServerRestartOfferQuery("sit1");
    expect(options.queryKey).toEqual(swimlaneKeys.configServerRestart("sit1"));
    await run(options);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.url.pathname).toBe("/api/v1/swimlanes/sit1/config-server-restart");
    expect(calls[0]?.init.method).toBe("GET");
    expect(calls[0]?.init.signal).toBeInstanceOf(AbortSignal);
  });

  it("sits under the swimlane's own key, so the events that refresh the swimlane (a finding, a pickup, a baseline) refresh it", () => {
    expect(swimlaneKeys.configServerRestart("sit1").slice(0, 2)).toEqual(swimlaneKeys.detail("sit1"));
    expect(swimlaneKeys.configServerRestart("sit1")).not.toEqual(swimlaneKeys.configServerRestart("sit2"));
  });

  it("is always read again when it is wanted, and not kept: the hub answers it no-store and a restart is decided on it", () => {
    const options = configServerRestartOfferQuery("sit1");
    expect(options.staleTime).toBe(0);
    expect(options.gcTime).toBe(0);
  });
});

describe("restartOfferQuery", () => {
  it("is keyed by the swimlane and the file, and asks for the file's offer", async () => {
    const options = restartOfferQuery("sit1", "tx-infinity-api/a b.yml");
    expect(options.queryKey).toEqual(swimlaneKeys.restartOffer("sit1", "tx-infinity-api/a b.yml"));
    await run(options);
    expect(calls[0]?.url.pathname).toBe("/api/v1/swimlanes/sit1/restart-offer");
    expect(calls[0]?.url.searchParams.get("path")).toBe("tx-infinity-api/a b.yml");
    expect(calls[0]?.init.method).toBe("GET");
  });

  it("is a different entry for each file, under the swimlane's own key", () => {
    expect(swimlaneKeys.restartOffer("sit1", "a.yml")).not.toEqual(swimlaneKeys.restartOffer("sit1", "b.yml"));
    expect(swimlaneKeys.restartOffer("sit1", "a.yml").slice(0, 2)).toEqual(swimlaneKeys.detail("sit1"));
  });

  it("is always read again when it is wanted, and not kept", () => {
    const options = restartOfferQuery("sit1", "a.yml");
    expect(options.staleTime).toBe(0);
    expect(options.gcTime).toBe(0);
  });
});

describe("what refreshes an offer that is on screen", () => {
  const events: readonly DomainEvent[] = [
    { type: "findings_changed", swimlane: "sit1" },
    { type: "pickup_changed", swimlane: "sit1", services: [] },
    { type: "pending_restart_changed", swimlane: "sit1", services: [] },
    { type: "swimlane_changed", swimlane: "sit1" },
    { type: "agent_status_changed", swimlane: "sit1", status: "connected" },
  ];
  const named = (event: DomainEvent, key: QueryKey): boolean => plan(event).targets.some((target) => matches(target, key, undefined));

  it.each(events.map((event) => [event.type, event] as const))("%s of its swimlane names both offers", (_type, event) => {
    expect(named(event, swimlaneKeys.configServerRestart("sit1"))).toBe(true);
    expect(named(event, swimlaneKeys.restartOffer("sit1", "a.yml"))).toBe(true);
  });

  it("does not name the offers of another swimlane", () => {
    expect(named({ type: "findings_changed", swimlane: "sit1" }, swimlaneKeys.configServerRestart("sit2"))).toBe(false);
    expect(named({ type: "findings_changed", swimlane: "sit1" }, swimlaneKeys.restartOffer("sit2", "a.yml"))).toBe(false);
  });
});
