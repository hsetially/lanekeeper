import { readFileSync } from "node:fs";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { DomainEvent } from "@/api/generated/model";
import { onSessionSignal, type SessionSignal } from "@/api/session-events";
import { BACKOFF_CAP_MS, backoffDelayMs } from "./backoff";
import { LiveStream, OFFLINE_AFTER_FAILURES, STALL_MS, type LiveSnapshot, type ResyncReason } from "./sse-client";
import { MAX_FRAME_CHARS } from "./sse-parser";
import { FakeHub, frame } from "@/test/fake-hub";

// The stream client against a hub that is a function of the test: it answers each connection with a response the test
// controls (frames pushed by hand, the stream ended or broken on purpose), under fake timers, so every delay is exact.
//
// PROVENANCE OF THE GOLDEN FRAMES. e2e/fixtures/sse-golden/*.txt were written BY HAND, by T1, from the format of the hub's
// crates/hub-platform/src/events/sse.rs. They were NOT captured from a live hub. They are the best statement of the wire
// format we have; these tests replay them verbatim.
const golden = (name: string): string => readFileSync(new URL(`../../e2e/fixtures/sse-golden/${name}`, import.meta.url), "utf8");

const FILE_OBSERVED = { type: "file_observed", swimlane: "sit1", path: "core/a.yml", hash: "a".repeat(64), source: "scan" } as const;
const FINDINGS = { type: "findings_changed", swimlane: "sit1" } as const;
// A random draw of 0.5 makes every delay the middle of its range, so tests can name them.
const random = () => 0.5;

type Harness = ReturnType<typeof setup>;

function setup(options: { stallMs?: number } = {}) {
  const hub = new FakeHub();
  const events: DomainEvent[] = [];
  const resyncs: ResyncReason[] = [];
  const problems: number[] = [];
  const snapshots: LiveSnapshot[] = [];
  const online = new EventTarget();
  const stream = new LiveStream({
    fetch: hub.fetch,
    random,
    onEvent: (e) => events.push(e),
    onResync: (r) => resyncs.push(r),
    onProblem: (p) => problems.push(p.status),
    onlineTarget: online,
    ...options,
  });
  stream.subscribe(() => snapshots.push(stream.getSnapshot()));
  return { hub, stream, events, resyncs, problems, snapshots, online };
}

/** Let promises and zero-delay timers settle. */
async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await vi.advanceTimersByTimeAsync(0);
}
async function advance(ms: number): Promise<void> {
  await vi.advanceTimersByTimeAsync(ms);
  await settle();
}
/** Start the stream, let the first connection answer, and push the hub's first frame so the stream is live. */
async function live(h: Harness): Promise<void> {
  h.stream.start();
  await settle();
  h.hub.last.push(golden("first-frame.txt"));
  await settle();
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2026-10-17T10:00:00Z"));
});
afterEach(() => {
  vi.useRealTimers();
});

describe("opening the stream", () => {
  it("makes one GET with Accept: text/event-stream, no cache, the session's credentials, and no Last-Event-ID the first time", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    expect(h.hub.connections).toHaveLength(1);
    const conn = h.hub.last;
    expect(conn.url).toBe("/api/v1/events");
    expect(conn.init.method).toBe("GET");
    expect(conn.headers.get("accept")).toBe("text/event-stream");
    expect(conn.headers.has("last-event-id")).toBe(false);
    expect(conn.init.cache).toBe("no-store");
    expect(conn.init.credentials).toBe("same-origin");
    expect(conn.init.redirect).toBe("error");
    expect(h.stream.getSnapshot().status).toBe("connecting");
  });

  it("is live from the first frame, and says when it last heard from the hub", async () => {
    const h = setup();
    await live(h);
    const snap = h.stream.getSnapshot();
    expect(snap.status).toBe("live");
    expect(snap.dataAsOf).toBe(Date.now());
    expect(snap.failures).toBe(0);
  });

  it("does nothing when started twice: one stream", async () => {
    const h = setup();
    h.stream.start();
    h.stream.start();
    await settle();
    expect(h.hub.connections).toHaveLength(1);
  });

  it("stop aborts the request, leaves nothing running and never opens another", async () => {
    const h = setup();
    await live(h);
    h.stream.stop();
    await advance(60_000);
    expect(h.hub.last.signal.aborted).toBe(true);
    expect(h.hub.connections).toHaveLength(1);
    expect(h.stream.getSnapshot().status).toBe("idle");
    expect(vi.getTimerCount()).toBe(0);
  });
});

describe("events", () => {
  it("delivers each event in order and remembers the id of the last one", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt") + golden("findings-changed.txt"));
    await settle();
    expect(h.events.map((e) => e.type)).toEqual(["file_observed", "findings_changed"]);
    expect(h.stream.getSnapshot().lastEventId).toBe("42");
  });

  it("takes an event that arrives split across reads", async () => {
    const h = setup();
    await live(h);
    const text = golden("file-observed.txt");
    h.hub.last.push(text.slice(0, 20));
    await settle();
    expect(h.events).toHaveLength(0);
    h.hub.last.push(text.slice(20));
    await settle();
    expect(h.events).toHaveLength(1);
  });

  it("tells the app to refetch on a resync frame, which has no id and does not move the resume point", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt"));
    await settle();
    h.resyncs.length = 0;
    h.hub.last.push(golden("resync.txt"));
    await settle();
    expect(h.resyncs).toEqual(["server"]);
    expect(h.stream.getSnapshot().lastEventId).toBe("41");
    expect(h.events.map((e) => e.type)).toEqual(["file_observed"]); // a resync is not handed on as an event
  });

  it("does not let a heartbeat move the resume point or count as an event", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("heartbeat.txt"));
    await settle();
    expect(h.events).toHaveLength(0);
    expect(h.stream.getSnapshot().lastEventId).toBeNull();
  });

  it("drops a frame it cannot trust and keeps reading: bad JSON, an unknown kind, a field of the wrong type, `event:` that disagrees with the data", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push("id: 50\nevent: file_observed\ndata: {not json\n\n");
    h.hub.last.push(`id: 51\nevent: something_new\ndata: ${JSON.stringify({ type: "something_new" })}\n\n`);
    h.hub.last.push(`id: 52\nevent: file_observed\ndata: ${JSON.stringify({ ...FILE_OBSERVED, path: 7 })}\n\n`);
    h.hub.last.push(`id: 53\nevent: findings_changed\ndata: ${JSON.stringify(FILE_OBSERVED)}\n\n`);
    h.hub.last.push(frame(54, FINDINGS));
    await settle();
    expect(h.events).toEqual([FINDINGS]);
    // Every frame was received, so the resume point is the last of them: the hub has nothing to replay for 50 to 53.
    expect(h.stream.getSnapshot().lastEventId).toBe("54");
    expect(h.stream.getSnapshot().status).toBe("live");
  });

  it("ignores an id that is not a number the hub would send (digits only, at most 19), and keeps the one before", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(frame(7, FINDINGS));
    h.hub.last.push(`id: abc\nevent: findings_changed\ndata: ${JSON.stringify(FINDINGS)}\n\n`);
    h.hub.last.push(`id: ${"9".repeat(20)}\nevent: findings_changed\ndata: ${JSON.stringify(FINDINGS)}\n\n`);
    h.hub.last.push(`id: 0\nevent: findings_changed\ndata: ${JSON.stringify(FINDINGS)}\n\n`);
    await settle();
    expect(h.events).toHaveLength(4);
    expect(h.stream.getSnapshot().lastEventId).toBe("7");
  });

  it("survives a handler that throws: the stream stays up and the next event arrives", async () => {
    const hub = new FakeHub();
    const seen: string[] = [];
    const stream = new LiveStream({
      fetch: hub.fetch,
      random,
      onEvent: (e) => {
        seen.push(e.type);
        if (e.type === "file_observed") throw new Error("a screen failed");
      },
      onResync: () => undefined,
      onProblem: () => undefined,
      onlineTarget: new EventTarget(),
    });
    stream.start();
    await settle();
    hub.last.push(golden("first-frame.txt") + golden("file-observed.txt") + golden("findings-changed.txt"));
    await settle();
    expect(seen).toEqual(["file_observed", "findings_changed"]);
    expect(stream.getSnapshot().status).toBe("live");
    expect(stream.getSnapshot().lastEventId).toBe("42");
  });
});

describe("resuming (Last-Event-ID)", () => {
  it("reconnects with the id of the last event and does not refetch: the hub replays what was missed", async () => {
    const h = setup();
    await live(h);
    expect(h.resyncs).toEqual(["no_id"]); // nothing to resume from yet: the first connect
    h.resyncs.length = 0;
    h.hub.last.push(golden("file-observed.txt") + golden("findings-changed.txt"));
    await settle();
    h.hub.last.fail();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    await advance(backoffDelayMs(0, { random }));
    expect(h.hub.connections).toHaveLength(2);
    expect(h.hub.last.headers.get("last-event-id")).toBe("42");
    h.hub.last.push(golden("first-frame.txt") + frame(43, FILE_OBSERVED));
    await settle();
    expect(h.stream.getSnapshot().status).toBe("live");
    expect(h.resyncs).toEqual([]);
    expect(h.events.map((e) => e.type)).toEqual(["file_observed", "findings_changed", "file_observed"]);
    expect(h.stream.getSnapshot().lastEventId).toBe("43");
  });

  it("refetches once on the first connect, because a connection without an id is not resumed (no replay, no resync)", async () => {
    const h = setup();
    await live(h);
    expect(h.resyncs).toEqual(["no_id"]);
    h.hub.last.push(golden("heartbeat.txt") + golden("heartbeat.txt"));
    await settle();
    expect(h.resyncs).toEqual(["no_id"]);
  });

  it("a routine rotation WITH an id refetches nothing", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt"));
    await settle();
    h.resyncs.length = 0;
    h.hub.last.push(golden("final-retry.txt"));
    h.hub.last.end(); // the hub closes the stream at its maximum age
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    await advance(6000); // the hub's `retry: 3000` is the floor; with jitter at most twice that
    expect(h.hub.connections).toHaveLength(2);
    expect(h.hub.last.headers.get("last-event-id")).toBe("41");
    h.hub.last.push(golden("first-frame.txt"));
    await settle();
    expect(h.stream.getSnapshot().status).toBe("live");
    expect(h.resyncs).toEqual([]);
  });

  it("a rotation with NO id (only heartbeats were seen) cannot be resumed, so the app refetches again", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("heartbeat.txt"));
    await settle();
    expect(h.resyncs).toEqual(["no_id"]);
    h.hub.last.push(golden("final-retry.txt"));
    h.hub.last.end();
    await settle();
    await advance(6000);
    expect(h.hub.connections).toHaveLength(2);
    expect(h.hub.last.headers.has("last-event-id")).toBe(false);
    h.hub.last.push(golden("first-frame.txt"));
    await settle();
    expect(h.resyncs).toEqual(["no_id", "no_id"]);
  });

  it("waits at least the hub's retry hint after it closed the stream cleanly, and not much more than twice that", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt") + "retry: 5000\n\n");
    await settle();
    h.hub.last.end();
    await settle();
    await advance(4900);
    expect(h.hub.connections).toHaveLength(1);
    await advance(5100);
    expect(h.hub.connections).toHaveLength(2);
  });

  it("forgets the resume point when told the user changed", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt"));
    await settle();
    h.stream.resetResumePoint();
    expect(h.stream.getSnapshot().lastEventId).toBeNull();
  });
});

describe("failing and backing off", () => {
  it("follows the ladder: each failed attempt waits the drawn delay of the next step", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    for (let attempt = 0; attempt < 6; attempt++) {
      h.hub.last.fail(); // the connection dies before it said anything
      await settle();
      const delay = backoffDelayMs(attempt, { random });
      expect(h.stream.getSnapshot().retryAt).toBe(Date.now() + delay);
      await advance(delay - 1);
      expect(h.hub.connections).toHaveLength(attempt + 1);
      await advance(1);
      expect(h.hub.connections).toHaveLength(attempt + 2);
    }
  });

  it("is reconnecting after one failure and offline after three in a row, and keeps trying at the cap", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    const statuses: string[] = [];
    for (let i = 0; i < OFFLINE_AFTER_FAILURES + 2; i++) {
      h.hub.last.fail();
      await settle();
      statuses.push(h.stream.getSnapshot().status);
      await advance(BACKOFF_CAP_MS * 2);
    }
    expect(statuses).toEqual(["reconnecting", "reconnecting", "offline", "offline", "offline"]);
    expect(h.hub.connections.length).toBeGreaterThanOrEqual(OFFLINE_AFTER_FAILURES + 2);
    // At the cap: the next attempt is at most 15 s away.
    const before = h.hub.connections.length;
    h.hub.last.fail();
    await settle();
    await advance(BACKOFF_CAP_MS);
    expect(h.hub.connections.length).toBe(before + 1);
  });

  it("while offline, the data stays 'as of' the last byte, and the first good frame afterwards asks the app to refetch", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt"));
    await settle();
    h.resyncs.length = 0;
    const lastHeard = Date.now();
    for (let i = 0; i < OFFLINE_AFTER_FAILURES; i++) {
      h.hub.last.fail();
      await settle();
      if (i < OFFLINE_AFTER_FAILURES - 1) await advance(BACKOFF_CAP_MS * 2);
    }
    expect(h.stream.getSnapshot().status).toBe("offline");
    expect(h.stream.getSnapshot().dataAsOf).toBe(lastHeard);
    await advance(BACKOFF_CAP_MS * 2);
    expect(h.stream.getSnapshot().dataAsOf).toBe(lastHeard);
    h.hub.last.push(golden("first-frame.txt"));
    await settle();
    expect(h.stream.getSnapshot().status).toBe("live");
    expect(h.stream.getSnapshot().failures).toBe(0);
    expect(h.resyncs).toEqual(["was_offline"]);
    expect(h.hub.last.headers.get("last-event-id")).toBe("41");
  });

  it("counts a 200 that is not an event stream (a proxy's login page) as a failure", async () => {
    const h = setup();
    h.hub.answer(() => new Response("<html>sign in</html>", { status: 200, headers: { "content-type": "text/html" } }));
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    expect(h.stream.getSnapshot().failures).toBe(1);
    expect(h.events).toHaveLength(0);
  });

  it.each([500, 502, 503, 504, 404, 408])("backs off and retries on a %i", async (status) => {
    const h = setup();
    h.hub.respond(status);
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    await advance(BACKOFF_CAP_MS);
    expect(h.hub.connections).toHaveLength(2);
  });

  it("waits at least what Retry-After says on a 429 (the sixth tab), and tries again", async () => {
    const h = setup();
    h.hub.respond(429, { type: "too_many_requests", headers: { "retry-after": "20" } });
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    await advance(19_000);
    expect(h.hub.connections).toHaveLength(1);
    await advance(21_000);
    expect(h.hub.connections).toHaveLength(2);
  });

  it("gives a connection that never opens the same treatment: a network error is a failure", async () => {
    const h = setup();
    h.hub.answer(() => {
      throw new TypeError("Failed to fetch");
    });
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    expect(h.stream.getSnapshot().failures).toBe(1);
  });
});

describe("the answers that end it (no retry)", () => {
  it("routes a 401 through the fetch layer's session handling and stops", async () => {
    const signals: SessionSignal[] = [];
    const off = onSessionSignal((s) => signals.push(s));
    const hub = new FakeHub();
    const stream = new LiveStream({ fetch: hub.fetch, random, onEvent: () => undefined, onResync: () => undefined, onlineTarget: new EventTarget() });
    hub.respond(401, { type: "unauthenticated" });
    stream.start();
    await settle();
    off();
    expect(signals).toEqual([{ kind: "unauthenticated" }]);
    expect(stream.getSnapshot()).toMatchObject({ status: "stopped", stopReason: "unauthorized" });
    await advance(120_000);
    expect(hub.connections).toHaveLength(1);
  });

  it.each([
    ["access-pending", "access_pending"],
    ["access_disabled", "access_disabled"],
  ])("routes a 403 %s to the session handler and stops", async (type, kind) => {
    const signals: SessionSignal[] = [];
    const off = onSessionSignal((s) => signals.push(s));
    const hub = new FakeHub();
    const stream = new LiveStream({ fetch: hub.fetch, random, onEvent: () => undefined, onResync: () => undefined, onlineTarget: new EventTarget() });
    hub.respond(403, { type });
    stream.start();
    await settle();
    off();
    expect(signals.map((s) => s.kind)).toEqual([kind]);
    expect(stream.getSnapshot()).toMatchObject({ status: "stopped", stopReason: "forbidden" });
    await advance(120_000);
    expect(hub.connections).toHaveLength(1);
  });

  it("stops on any other 403 as well, with nothing routed", async () => {
    const h = setup();
    h.hub.respond(403, { type: "forbidden" });
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot()).toMatchObject({ status: "stopped", stopReason: "forbidden" });
    await advance(120_000);
    expect(h.hub.connections).toHaveLength(1);
  });

  it("stops on a 400: the request is wrong and asking again will not make it right", async () => {
    const h = setup();
    h.hub.respond(400, { type: "bad_request" });
    h.stream.start();
    await settle();
    expect(h.stream.getSnapshot()).toMatchObject({ status: "stopped", stopReason: "bad_request" });
    await advance(120_000);
    expect(h.hub.connections).toHaveLength(1);
  });

  it("hands the problem to the owner of the session, and never the response body to anyone else", async () => {
    const h = setup();
    h.hub.respond(401, { type: "unauthenticated" });
    h.stream.start();
    await settle();
    expect(h.problems).toEqual([401]);
  });
});

describe("a stream that goes quiet", () => {
  it("is dropped after 45 s without a byte, and counts as a failure", async () => {
    expect(STALL_MS).toBe(45_000);
    const h = setup();
    await live(h);
    await advance(STALL_MS - 1000);
    expect(h.hub.last.signal.aborted).toBe(false);
    await advance(2000);
    expect(h.hub.connections[0]?.signal.aborted).toBe(true);
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    expect(h.stream.getSnapshot().failures).toBe(1);
  });

  it("is kept by heartbeats: the hub sends one every 15 s", async () => {
    const h = setup();
    await live(h);
    for (let i = 0; i < 12; i++) {
      await advance(15_000);
      h.hub.last.push(golden("heartbeat.txt"));
      await settle();
    }
    expect(h.hub.connections).toHaveLength(1);
    expect(h.stream.getSnapshot().status).toBe("live");
    expect(h.stream.getSnapshot().dataAsOf).toBe(Date.now());
  });
});

describe("limits", () => {
  it("drops a stream that sends a frame over 64 KiB, hands nothing on, and reconnects", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(`data: ${"x".repeat(MAX_FRAME_CHARS + 1)}\n\n`);
    await settle();
    expect(h.events).toHaveLength(0);
    expect(h.hub.connections[0]?.signal.aborted).toBe(true);
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
  });
});

describe("one stream at a time", () => {
  it("retryNow while reconnecting opens the next connection at once", async () => {
    const h = setup();
    await live(h);
    h.hub.last.fail();
    await settle();
    expect(h.stream.getSnapshot().status).toBe("reconnecting");
    h.stream.retryNow();
    await settle();
    expect(h.hub.connections).toHaveLength(2);
    expect(h.hub.open).toBe(1);
  });

  it("retryNow while live does nothing", async () => {
    const h = setup();
    await live(h);
    h.stream.retryNow();
    await settle();
    expect(h.hub.connections).toHaveLength(1);
  });

  it("reconnect (a role change) closes the open stream, opens one at once with the same resume point, and is no failure", async () => {
    const h = setup();
    await live(h);
    h.hub.last.push(golden("file-observed.txt"));
    await settle();
    h.resyncs.length = 0;
    h.stream.reconnect();
    await settle();
    expect(h.hub.connections).toHaveLength(2);
    expect(h.hub.connections[0]?.signal.aborted).toBe(true);
    expect(h.hub.open).toBe(1);
    expect(h.hub.last.headers.get("last-event-id")).toBe("41");
    h.hub.last.push(golden("first-frame.txt"));
    await settle();
    expect(h.stream.getSnapshot()).toMatchObject({ status: "live", failures: 0 });
    expect(h.resyncs).toEqual([]);
  });

  it("never has two streams open, whatever the order of failures, restarts and retries", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    for (let i = 0; i < 40; i++) {
      const pick = i % 5;
      if (pick === 0) h.hub.last.fail();
      else if (pick === 1) h.stream.reconnect();
      else if (pick === 2) h.stream.retryNow();
      else if (pick === 3) h.hub.last.end();
      await advance(1500);
      expect(h.hub.open, `after step ${i}`).toBeLessThanOrEqual(1);
    }
  });

  it("opens again when the browser says it is back online", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    h.hub.last.fail();
    await settle();
    h.online.dispatchEvent(new Event("online"));
    await settle();
    expect(h.hub.connections).toHaveLength(2);
  });

  it("stops listening for it when stopped", async () => {
    const h = setup();
    h.stream.start();
    await settle();
    h.hub.last.fail();
    await settle();
    h.stream.stop();
    h.online.dispatchEvent(new Event("online"));
    await settle();
    expect(h.hub.connections).toHaveLength(1);
  });
});

describe("what the UI can read", () => {
  it("is a stable snapshot between changes (what useSyncExternalStore needs) and tells subscribers about each change", async () => {
    const h = setup();
    const first = h.stream.getSnapshot();
    expect(h.stream.getSnapshot()).toBe(first);
    h.stream.start();
    await settle();
    expect(h.snapshots.length).toBeGreaterThan(0);
    const connecting = h.stream.getSnapshot();
    expect(connecting).not.toBe(first);
    expect(h.stream.getSnapshot()).toBe(connecting);
  });

  it("stops telling a subscriber that unsubscribed", async () => {
    const h = setup();
    const calls: number[] = [];
    const off = h.stream.subscribe(() => calls.push(1));
    h.stream.start();
    await settle();
    const n = calls.length;
    off();
    h.hub.last.push(golden("first-frame.txt"));
    await settle();
    expect(calls.length).toBe(n);
  });
});
