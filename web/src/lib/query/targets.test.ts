import { QueryObserver } from "@tanstack/react-query";
import { describe, expect, it } from "vitest";
import { createQueryClient } from "./client";
import { ASKED_BY_HAND, invalidateTargets } from "./targets";

// `ASKED_BY_HAND` is the one way a query opts out of a bulk refetch (a resync). The rest of the cache must not notice it, and a change
// that names the query must still reach it: a query that is asked for by hand is not a query that is never refreshed.

function watch(client: ReturnType<typeof createQueryClient>, key: readonly unknown[], meta?: Record<string, unknown>) {
  let reads = 0;
  const observer = new QueryObserver(client, {
    queryKey: key,
    queryFn: () => Promise.resolve((reads += 1)),
    staleTime: 60_000,
    ...(meta === undefined ? {} : { meta }),
  });
  const stop = observer.subscribe(() => undefined);
  return { reads: () => reads, stop, ready: () => observer.getCurrentResult().status === "success" };
}

describe("invalidateTargets, and a query that is asked for by hand", () => {
  it("leaves it out of a resync (`everything`) and reads every other active query again", async () => {
    const client = createQueryClient();
    const asked = watch(client, ["probe-asked", "sit1"], ASKED_BY_HAND);
    const pushed = watch(client, ["probe-pushed", "sit1"]);
    const other = watch(client, ["probe-other", "sit1"], { somethingElse: true });
    await expect.poll(() => asked.ready() && pushed.ready() && other.ready()).toBe(true);
    await invalidateTargets(client, "everything");
    expect(pushed.reads(), "the control is read again, so this is not vacuous").toBe(2);
    expect(other.reads(), "meta that is not ours does not exempt a query").toBe(2);
    expect(asked.reads(), "the one asked for by hand is not read again").toBe(1);
    expect(client.getQueryState(["probe-asked", "sit1"])?.isInvalidated, "and not even marked stale").toBe(false);
    for (const w of [asked, pushed, other]) w.stop();
    client.clear();
  });

  it("is still reached by a target that names it", async () => {
    const client = createQueryClient();
    const asked = watch(client, ["probe-asked", "sit1"], ASKED_BY_HAND);
    await expect.poll(() => asked.ready()).toBe(true);
    await invalidateTargets(client, [{ kind: "key", key: ["probe-asked", "sit1"] }]);
    expect(asked.reads()).toBe(2);
    asked.stop();
    client.clear();
  });

  it("is a flag only a literal `true` sets", async () => {
    const client = createQueryClient();
    const falsy = watch(client, ["probe-falsy", "sit1"], { askedByHand: "yes" });
    await expect.poll(() => falsy.ready()).toBe(true);
    await invalidateTargets(client, "everything");
    expect(falsy.reads()).toBe(2);
    falsy.stop();
    client.clear();
  });
});
