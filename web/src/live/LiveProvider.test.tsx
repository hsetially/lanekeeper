import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import { act, render, screen } from "@testing-library/react";
import { StrictMode, type ReactElement, type ReactNode } from "react";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { onSessionSignal, type SessionSignal } from "@/api/session-events";
import type { Me, Role } from "@/api/generated/model";
import { SessionContext, type SessionState } from "@/auth/session";
import { useWriteGate } from "@/components/app/write-gate";
import { swimlaneKeys } from "@/lib/query/keys";
import { FakeHub, frame } from "@/test/fake-hub";
import { FILE_OBSERVED_GOLDEN, FINDINGS_GOLDEN, FIRST_FRAME, HEARTBEAT } from "@/test/sse-golden";
import { LiveProvider, useLiveActions, useLiveNotices, useLiveStatus } from "./LiveProvider";
import type { Notice } from "./invalidation";
import { WINDOW_MS } from "./apply";
import { OFFLINE_AFTER_FAILURES } from "./sse-client";

// The provider mounted at the router root: one stream per tab, only for a signed-in user who has a role, closed when the
// session goes, reconnected when the role changes, and the source of the "online" that turns writes off.

const ME = (id = "u1", role: Role = "viewer") => ({ id, email: "ada@example.test", display_name: "Ada", role }) as unknown as Me;
const ready = (role: Role | null, id = "u1"): SessionState => ({ status: "ready", me: ME(id, role ?? "viewer"), role });

let hub: FakeHub;
let client: QueryClient;
const random = () => 0.5;

beforeEach(() => {
  vi.useFakeTimers();
  hub = new FakeHub();
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});
afterEach(() => {
  client.clear();
  vi.useRealTimers();
});

function Tree({ session, children }: { session: SessionState; children?: ReactNode }): ReactElement {
  return (
    <QueryClientProvider client={client}>
      <SessionContext.Provider value={session}>
        <LiveProvider config={{ stream: { fetch: hub.fetch, random, onlineTarget: new EventTarget() } }}>{children ?? <Status />}</LiveProvider>
      </SessionContext.Provider>
    </QueryClientProvider>
  );
}

function Status() {
  const live = useLiveStatus();
  return <output data-testid="status">{live.status}</output>;
}
const status = () => screen.getByTestId("status").textContent;

// The engine is a chunk of its own, imported when a signed-in user with a role is on screen; warm in the module cache, so that
// the import resolves in the next turn of the event loop that the fake timers yield.
beforeAll(async () => {
  await import("./live");
});

async function tick(ms = 5): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}
async function open(): Promise<void> {
  await tick();
  await act(async () => {
    hub.last.push(FIRST_FRAME);
    await vi.advanceTimersByTimeAsync(0);
  });
}

describe("when the stream is open", () => {
  it.each<[string, SessionState]>([
    ["no session yet (a public screen)", { status: "none" }],
    ["a session that is loading", { status: "loading" }],
    ["a session that failed", { status: "error", problem: {} as never }],
    ["a user with no role yet", ready(null)],
  ])("is not opened for %s", async (_name, session) => {
    render(<Tree session={session} />);
    await tick(10_000);
    expect(hub.connections).toHaveLength(0);
    expect(status()).toBe("idle");
  });

  it("is opened once for a user with a role, and is live from the first frame", async () => {
    render(<Tree session={ready("viewer")} />);
    await tick();
    expect(hub.connections).toHaveLength(1);
    expect(hub.last.url).toBe("/api/v1/events");
    expect(status()).toBe("connecting");
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(status()).toBe("live");
  });

  it("is one stream under React's development double effects: the first start is cancelled before it connects", async () => {
    // StrictMode must be the root element: React double-invokes effects only for a strict root.
    render(
      <StrictMode>
        <Tree session={ready("viewer")} />
      </StrictMode>,
    );
    await tick();
    expect(hub.connections).toHaveLength(1);
    expect(hub.open).toBe(1);
  });

  it("stays one stream across re-renders, navigation (new children) and a same-role session object", async () => {
    const view = render(<Tree session={ready("viewer")} />);
    await open();
    for (let i = 0; i < 5; i++) {
      view.rerender(<Tree session={ready("viewer")}>{<Status key={i} />}</Tree>);
      await tick(1000);
    }
    expect(hub.connections).toHaveLength(1);
    expect(hub.open).toBe(1);
  });

  it("is closed when the session goes away (sign-out, a 401, a public screen), and not opened again by a re-render", async () => {
    const view = render(<Tree session={ready("viewer")} />);
    await open();
    view.rerender(<Tree session={{ status: "none" }} />);
    await tick(5000);
    expect(hub.open).toBe(0);
    expect(status()).toBe("idle");
    view.rerender(<Tree session={{ status: "none" }} />);
    await tick(5000);
    expect(hub.connections).toHaveLength(1);
  });

  it("is closed when the provider unmounts, and leaves no timer behind", async () => {
    const view = render(<Tree session={ready("viewer")} />);
    await open();
    view.unmount();
    await tick(1000);
    expect(hub.open).toBe(0);
    expect(vi.getTimerCount()).toBe(0);
  });

  it("is reconnected, not doubled, when the role changes: the hub filters by role at connect time", async () => {
    const view = render(<Tree session={ready("viewer")} />);
    await open();
    await act(async () => {
      hub.last.push(frame(41, { type: "findings_changed", swimlane: "sit1" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    view.rerender(<Tree session={ready("admin")} />);
    await tick();
    expect(hub.connections).toHaveLength(2);
    expect(hub.open).toBe(1);
    expect(hub.connections[0]?.signal.aborted).toBe(true);
    expect(hub.last.headers.get("last-event-id")).toBe("41");
  });

  it("starts from nothing when the user is a different one: another person's resume point means nothing", async () => {
    const view = render(<Tree session={ready("viewer", "u1")} />);
    await open();
    await act(async () => {
      hub.last.push(frame(41, { type: "findings_changed", swimlane: "sit1" }));
      await vi.advanceTimersByTimeAsync(0);
    });
    view.rerender(<Tree session={ready("viewer", "u2")} />);
    await tick();
    expect(hub.connections).toHaveLength(2);
    expect(hub.open).toBe(1);
    expect(hub.last.headers.has("last-event-id")).toBe(false);
  });

  it("does not open a stream again after a 401: the session handler is signing the user out", async () => {
    const signals: SessionSignal[] = [];
    const off = onSessionSignal((s) => signals.push(s));
    hub.respond(401, { type: "unauthenticated" });
    const view = render(<Tree session={ready("viewer")} />);
    await tick();
    expect(status()).toBe("stopped");
    expect(signals).toEqual([{ kind: "unauthenticated" }]);
    view.rerender(<Tree session={ready("viewer")} />);
    await tick(60_000);
    off();
    expect(hub.connections).toHaveLength(1);
  });
});

describe("what a stream does to the queries", () => {
  function Watch({ id }: { id: string }) {
    useQuery({ queryKey: swimlaneKeys.detail(id), queryFn: () => Promise.resolve({ id }), staleTime: Infinity });
    return null;
  }
  const fetched = () => client.getQueryState(swimlaneKeys.detail("sit1"))?.dataUpdateCount ?? 0;

  it("refetches the queries on screen once when it first connects (nothing to resume from), and not for heartbeats", async () => {
    render(
      <Tree session={ready("viewer")}>
        <Watch id="sit1" />
      </Tree>,
    );
    await tick();
    const before = fetched();
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    });
    expect(fetched()).toBe(before + 1);
    await act(async () => {
      hub.last.push(HEARTBEAT + HEARTBEAT);
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(fetched()).toBe(before + 1);
  });

  it("refetches the query an event names, once, after the window", async () => {
    render(
      <Tree session={ready("viewer")}>
        <Watch id="sit1" />
        <Watch id="sit2" />
      </Tree>,
    );
    await open();
    await tick(WINDOW_MS + 1);
    const sit1 = fetched();
    const sit2 = client.getQueryState(swimlaneKeys.detail("sit2"))?.dataUpdateCount ?? 0;
    await act(async () => {
      hub.last.push(frame(41, { type: "findings_changed", swimlane: "sit1" }) + frame(42, { type: "findings_changed", swimlane: "sit1" }));
      await vi.advanceTimersByTimeAsync(WINDOW_MS + 1);
    });
    expect(fetched()).toBe(sit1 + 1);
    expect(client.getQueryState(swimlaneKeys.detail("sit2"))?.dataUpdateCount ?? 0).toBe(sit2);
  });

  it("hands each notice to a listener, and stops when the listener's component is gone", async () => {
    const notices: Notice[] = [];
    function Listener() {
      useLiveNotices((n) => notices.push(n));
      return null;
    }
    const view = render(
      <Tree session={ready("viewer")}>
        <Listener />
      </Tree>,
    );
    await open();
    await act(async () => {
      hub.last.push(FILE_OBSERVED_GOLDEN);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(notices).toEqual([{ kind: "out-of-band-change", swimlane: "sit1", path: "core/core-sit1.yml" }]);
    view.rerender(<Tree session={ready("viewer")}>{<Status />}</Tree>);
    await act(async () => {
      hub.last.push(FINDINGS_GOLDEN);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(notices).toHaveLength(1);
  });
});

describe("what the UI reads", () => {
  function Gate() {
    const gate = useWriteGate("viewer");
    return <output data-testid="gate">{gate.allowed ? "allowed" : (gate.reason ?? "no")}</output>;
  }
  const gate = () => screen.getByTestId("gate").textContent;

  it("turns writes off while offline (three failures in a row), and back on when the stream is back", async () => {
    render(
      <Tree session={ready("viewer")}>
        <Status />
        <Gate />
      </Tree>,
    );
    await tick();
    expect(gate()).toBe("allowed");
    for (let i = 0; i < OFFLINE_AFTER_FAILURES; i++) {
      await act(async () => {
        hub.last.fail();
        await vi.advanceTimersByTimeAsync(0);
      });
      if (i < OFFLINE_AFTER_FAILURES - 1) await tick(20_000);
    }
    expect(status()).toBe("offline");
    expect(gate()).toBe("Unavailable offline");
    await tick(20_000);
    await act(async () => {
      hub.last.push(FIRST_FRAME);
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(status()).toBe("live");
    expect(gate()).toBe("allowed");
  });

  it("keeps writes on while the stream is merely reconnecting: one dropped connection is not a reason to lock the UI", async () => {
    render(
      <Tree session={ready("viewer")}>
        <Status />
        <Gate />
      </Tree>,
    );
    await open();
    await act(async () => {
      hub.last.fail();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(status()).toBe("reconnecting");
    expect(gate()).toBe("allowed");
  });

  it("lets a component ask for an attempt now", async () => {
    function Retry() {
      const { retryNow } = useLiveActions();
      return <button onClick={retryNow}>retry</button>;
    }
    render(
      <Tree session={ready("viewer")}>
        <Status />
        <Retry />
      </Tree>,
    );
    await open();
    await act(async () => {
      hub.last.fail();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(hub.connections).toHaveLength(1);
    await act(async () => {
      screen.getByRole("button", { name: "retry" }).click();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(hub.connections).toHaveLength(2);
  });

  it("does not re-render a component that reads the status for every event: only for a change of status", async () => {
    let renders = 0;
    function Counter() {
      useLiveStatus();
      renders += 1;
      return null;
    }
    render(
      <Tree session={ready("viewer")}>
        <Counter />
      </Tree>,
    );
    await open();
    const before = renders;
    await act(async () => {
      for (let i = 1; i <= 50; i++) hub.last.push(frame(i, { type: "findings_changed", swimlane: "sit1" }));
      await vi.advanceTimersByTimeAsync(1000);
    });
    expect(renders).toBe(before);
  });

  it("answers 'idle' and does nothing without a provider (a test of another component, a public screen)", () => {
    function Bare() {
      const live = useLiveStatus();
      const { retryNow } = useLiveActions();
      retryNow();
      return <output data-testid="bare">{live.status}</output>;
    }
    render(<Bare />);
    expect(screen.getByTestId("bare").textContent).toBe("idle");
  });
});

describe("when the provider is switched off (a test of something else)", () => {
  it("never opens a stream, whatever the session says", async () => {
    render(
      <QueryClientProvider client={client}>
        <SessionContext.Provider value={ready("admin")}>
          <LiveProvider config={{ enabled: false, stream: { fetch: hub.fetch } }}>
            <Status />
          </LiveProvider>
        </SessionContext.Provider>
      </QueryClientProvider>,
    );
    await tick(60_000);
    expect(hub.connections).toHaveLength(0);
    expect(status()).toBe("idle");
  });
});
